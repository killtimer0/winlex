use std::{
    path::PathBuf,
    sync::{
        OnceLock,
        atomic::{AtomicBool, Ordering},
    },
};
use winlex_core::{Architecture, Catalog, EntityCategory, SearchMode, SearchOptions, SearchStep};

include!("../../../tests/rust_search.rs");

fn catalog() -> &'static Catalog {
    static DB: OnceLock<Catalog> = OnceLock::new();
    DB.get_or_init(|| {
        let path = std::env::var_os("WINLEX_TEST_WINMD")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../inputs/Windows.Win32.winmd")
            });
        Catalog::open_many([path]).unwrap()
    })
}

#[test]
fn search_categories_partition_results_without_name_conventions() {
    let db = catalog();
    let all: Vec<_> = search_results(db.search("e", SearchOptions::default())).collect();
    assert!(all.iter().any(|hit| hit.id.token >> 24 == 1));
    for mask in [
        0,
        1,
        2,
        4,
        8,
        16,
        32,
        64,
        128,
        EntityCategory::DEFAULT,
        EntityCategory::ALL,
    ] {
        let expected: Vec<_> = all
            .iter()
            .filter(|hit| db.declaration(hit.id, None).unwrap().category() as u32 & mask != 0)
            .map(|hit| hit.id)
            .collect();
        let actual = search_results(db.search(
            "e",
            SearchOptions {
                categories: Some(mask),
                ..Default::default()
            },
        ))
        .map(|hit| hit.id)
        .collect::<Vec<_>>();
        assert_eq!(actual, expected, "mask {mask}");
    }
}

#[test]
fn index_keeps_types_and_members_uniformly() {
    for (name, category) in [
        (
            "Windows.Win32.Storage.FileSystem.Apis",
            EntityCategory::Type,
        ),
        (
            "Windows.Win32.Storage.FileSystem.Apis.CreateFileW",
            EntityCategory::Function,
        ),
        ("Windows.Win32.Foundation.POINT.x", EntityCategory::Field),
    ] {
        let results: Vec<_> = search_results(catalog().search(
            name,
            SearchOptions {
                mode: SearchMode::Name,
                categories: Some(category as u32),
                ..Default::default()
            },
        ))
        .collect();
        assert!(!results.is_empty(), "{name}");
        assert!(results.iter().all(|hit| hit.full_name == name));
    }
}

#[test]
fn stepped_and_iterated_search_share_progress_and_cancellation() {
    let db = catalog();
    let collect = || {
        search_results(db.search("CrtFlW", SearchOptions::default()))
            .map(|hit| hit.id)
            .collect::<Vec<_>>()
    };
    let expected = collect();
    assert!(!expected.is_empty());
    let mut search = db.search("CrtFlW", SearchOptions::default()).unwrap();
    let mut stepped = vec![];
    let mut skipped = false;
    loop {
        match search.step().unwrap() {
            SearchStep::Match(hit) => stepped.push(hit.id),
            SearchStep::Skipped => skipped = true,
            SearchStep::Done => break,
        }
    }
    assert!(skipped);
    assert_eq!(stepped, expected);
    assert!(matches!(search.step().unwrap(), SearchStep::Done));
    assert!(search.next().is_none());
    let mut mixed = db.search("CrtFlW", SearchOptions::default()).unwrap();
    let first = loop {
        match mixed.step().unwrap() {
            SearchStep::Match(hit) => break hit.id,
            SearchStep::Skipped => {}
            SearchStep::Done => panic!("missing fuzzy match"),
        }
    };
    assert_eq!(first, expected[0]);
    assert_eq!(
        mixed.map(|hit| hit.unwrap().id).collect::<Vec<_>>(),
        expected[1..]
    );

    let cancel = AtomicBool::new(false);
    let mut search = db
        .search(
            "CrtFlW",
            SearchOptions {
                cancel: Some(&cancel),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(!matches!(search.step().unwrap(), SearchStep::Done));
    cancel.store(true, Ordering::Relaxed);
    assert!(search.step().is_err());
    assert!(matches!(search.step().unwrap(), SearchStep::Done));
    assert!(search.next().is_none());
    let mut cancelled = db
        .search(
            "__no_such_symbol__",
            SearchOptions {
                cancel: Some(&cancel),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(cancelled.next().unwrap().is_err());
    assert!(cancelled.next().is_none());

    cancel.store(false, Ordering::Relaxed);
    let mut iterated = db
        .search(
            "file",
            SearchOptions {
                cancel: Some(&cancel),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(iterated.next().unwrap().is_ok());
    cancel.store(true, Ordering::Relaxed);
    assert!(iterated.next().unwrap().is_err());
    assert!(iterated.next().is_none());
}

#[test]
fn calculations_use_selected_architecture() {
    let db = catalog();
    let type_id = |name, arch| {
        first_match(db.search_exact(
            name,
            SearchOptions {
                architecture: Some(arch),
                categories: Some(EntityCategory::Type as u32),
                ..Default::default()
            },
        ))
        .id
    };
    let layout = |name, arch| db.layout(type_id(name, arch), arch).unwrap();
    assert_eq!(layout("POINT", Architecture::X64).size_bits, 64);
    let dcb = layout("DCB", Architecture::X64);
    assert!(
        db.entity(dcb.id)
            .unwrap()
            .full_name
            .ends_with("Communication.DCB")
    );
    let backing = dcb.fields.iter().find(|f| f.name == "_bitfield").unwrap();
    let bit = backing
        .bitfields
        .iter()
        .find(|b| b.name == "fDtrControl")
        .unwrap();
    assert_eq!(backing.offset_bits + bit.relative_offset_bits, 68);
    assert_eq!(bit.width_bits, 2);
    assert_eq!(layout("OPENFILENAMEW", Architecture::X86).size_bits, 704);
    assert_eq!(layout("OPENFILENAMEW", Architecture::X64).size_bits, 1216);
    let ov = layout("OVERLAPPED", Architecture::X64);
    assert_eq!(ov.size_bits, 256);
    let union = ov.fields.iter().find(|f| f.name == "Anonymous").unwrap();
    assert_eq!(union.offset_bits, 128);
    assert!(!union.fields.is_empty());
    let vt = db
        .vtable(type_id("IStream", Architecture::X64), Architecture::X64)
        .unwrap();
    assert_eq!(vt.slots[5].method, "Seek");
    assert_eq!(vt.slots[13].method, "Clone");
}
