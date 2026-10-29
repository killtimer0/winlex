//! Integration tests for the CLI adapter and query library.
use serde_json::Value;
use std::{
    ffi::OsString,
    path::PathBuf,
    sync::{Arc, OnceLock, atomic::AtomicBool},
};
use winlex_core::{Architecture, Catalog, SearchMode, SearchOptions};

fn input() -> PathBuf {
    if let Some(path) = std::env::var_os("WINLEX_TEST_WINMD") {
        return path.into();
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../inputs/Windows.Win32.winmd")
}
fn entity_id(v: &Value) -> winlex_core::EntityId {
    winlex_core::EntityId {
        source_id: v["source_id"].as_u64().unwrap() as usize,
        token: v["token"].as_u64().unwrap() as u32,
    }
}
fn catalog() -> &'static Catalog {
    static DB: OnceLock<Catalog> = OnceLock::new();
    DB.get_or_init(|| Catalog::open(input()).expect("prepare pinned inputs/Windows.Win32.winmd"))
}

#[test]
fn search_mode_flags_are_exclusive_and_only_apply_to_search() {
    for arguments in [
        vec!["search", "1", "--name", "--value"],
        vec!["search", "1", "--value", "--guid"],
        vec!["search", "1", "--guid", "--name"],
        vec!["search", "1", "--name", "--name"],
        vec!["show", "CreateFileW", "--name"],
        vec!["summary", "--value"],
        vec!["search", "CreateFileW", "--exact"],
        vec!["value", "1"],
        vec!["guid", "1234"],
    ] {
        let (code, result, _) = cli(&arguments);
        assert_eq!(code, 2, "{arguments:?}");
        assert_eq!(result["schema"], "winlex-cli-6");
        assert_eq!(result["ok"], false);
    }
    for (query, flag) in [
        ("08", "--value"),
        ("NaN", "--value"),
        ("FF 239", "--guid"),
        (".", "--name"),
    ] {
        let (code, result, _) = cli(&["search", query, flag]);
        assert_eq!(code, 3, "{query}");
        assert_eq!(result["ok"], false);
        assert_eq!(cli(&["search", query]).0, 0);
    }
}

#[test]
fn guid_fragments_bytes_and_numeric_radices_share_the_cli_search_contract() {
    let (_, expected, _) = cli(&["search", "2147483648", "--value", "--limit", "1000"]);
    for query in [
        "+0x80000000",
        "0b10000000000000000000000000000000",
        "020000000000",
        "0o20000000000",
    ] {
        let (code, result, _) = cli(&["search", query, "--value", "--limit", "1000"]);
        assert_eq!(code, 0, "{query}");
        assert_eq!(result["result"], expected["result"], "{query}");
    }
    let (_, expected, _) = cli(&[
        "search",
        "0000000c00000000c000000000000046",
        "--guid",
        "--limit",
        "1000",
    ]);
    for query in [
        "{0000000C-0000-0000-C000-000000000046}",
        "0,0,0,12,0,0,0,0,192,0,0,0,0,0,0,70",
    ] {
        let (code, result, _) = cli(&["search", query, "--guid", "--limit", "1000"]);
        assert_eq!(code, 0);
        assert_eq!(result["result"], expected["result"]);
    }
    let (_, scope, _) = cli(&["search", "Windows.Win32.Storage.FileSystem.", "--name"]);
    assert!(!scope["result"]["matches"].as_array().unwrap().is_empty());
    assert_eq!(cli(&["show", "Windows.Win32.Storage.FileSystem."]).0, 3);
}

#[test]
fn search_categories_partition_results_without_name_conventions() {
    use winlex_core::EntityCategory;
    let db = catalog();
    let all: Vec<_> = db
        .search("e", SearchOptions::default())
        .unwrap()
        .map(|item| item.unwrap().id)
        .collect();
    assert!(all.iter().any(|id| id.token >> 24 == 1));
    for mask in [
        0,
        1,
        2,
        4,
        8,
        16,
        32,
        64,
        EntityCategory::DEFAULT,
        EntityCategory::ALL,
    ] {
        let expected: Vec<_> = all
            .iter()
            .copied()
            .filter(|&id| db.declaration(id, None).unwrap().category() as u32 & mask != 0)
            .collect();
        let actual: Vec<_> = db
            .search(
                "e",
                SearchOptions {
                    categories: Some(mask),
                    ..Default::default()
                },
            )
            .unwrap()
            .map(|item| item.unwrap().id)
            .collect();
        assert_eq!(actual, expected, "mask {mask}");
    }
    let methods: Vec<_> = db
        .search(
            "CreateFileW",
            SearchOptions {
                categories: Some(EntityCategory::Method as u32),
                ..Default::default()
            },
        )
        .unwrap()
        .map(|item| item.unwrap())
        .collect();
    assert!(!methods.is_empty());
    assert!(methods.iter().all(|item| item.kind == "method"));
}

#[test]
fn core_index_keeps_types_and_indexes_members_uniformly() {
    use winlex_core::EntityCategory;
    for (name, category) in [
        (
            "Windows.Win32.Storage.FileSystem.Apis",
            EntityCategory::Type,
        ),
        (
            "Windows.Win32.Storage.FileSystem.Apis.CreateFileW",
            EntityCategory::Method,
        ),
        ("Windows.Win32.Foundation.POINT.x", EntityCategory::Field),
    ] {
        let results: Vec<_> = catalog()
            .search(
                name,
                SearchOptions {
                    mode: SearchMode::Name,
                    categories: Some(category as u32),
                    ..Default::default()
                },
            )
            .unwrap()
            .map(|item| item.unwrap())
            .collect();
        assert!(!results.is_empty(), "{name}");
        assert!(results.iter().all(|item| item.full_name == name));
    }
}

#[test]
fn queries_require_explicit_inputs_but_help_and_version_do_not() {
    for args in [vec!["summary"], vec!["search", "CreateFileW", "--json"]] {
        let (mut out, mut err) = (vec![], vec![]);
        assert_eq!(
            winlex_cli::run(args.iter().map(OsString::from), &mut out, &mut err),
            2
        );
        if args.contains(&"--json") {
            let response: Value = serde_json::from_slice(&out).unwrap();
            assert_eq!(response["error"]["code"], "invalid_arguments");
            assert!(
                response["error"]["message"]
                    .as_str()
                    .unwrap()
                    .contains("input file")
            );
            assert!(err.is_empty());
        } else {
            assert!(out.is_empty());
            assert!(String::from_utf8(err).unwrap().contains("input file"));
        }
    }
    for option in ["--help", "--version"] {
        let (mut out, mut err) = (vec![], vec![]);
        assert_eq!(
            winlex_cli::run([OsString::from(option)], &mut out, &mut err),
            0
        );
        assert!(!out.is_empty());
        assert!(err.is_empty());
    }
}

#[test]
fn streaming_search_is_complete_deterministic_and_cancellable() {
    use std::sync::atomic::Ordering;
    let db = catalog();
    let collect = || {
        db.search("file", SearchOptions::default())
            .unwrap()
            .map(|e| e.map(|e| e.id))
            .collect::<winlex_core::Result<Vec<_>>>()
            .unwrap()
    };
    let ids = collect();
    assert!(ids.len() > 100);
    assert_eq!(ids, collect());
    let mut ranked = db
        .search("file", SearchOptions::default())
        .unwrap()
        .collect::<winlex_core::Result<Vec<_>>>()
        .unwrap();
    ranked.sort_unstable_by(winlex_core::SearchMatch::compare);
    let ids: Vec<_> = ranked.iter().map(|m| m.id).collect();
    let (_, page, _) = cli(&["search", "file", "--offset", "17", "--limit", "25"]);
    assert_eq!(
        page["result"]["total"].as_u64().unwrap() as usize,
        ids.len()
    );
    let paged: Vec<_> = page["result"]["matches"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| entity_id(&e["id"]))
        .collect();
    assert_eq!(paged, ids[17..42]);
    let cancel = AtomicBool::new(false);
    let mut results = db
        .search(
            "file",
            SearchOptions {
                cancel: Some(&cancel),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(results.next().unwrap().is_ok());
    cancel.store(true, Ordering::Relaxed);
    assert!(results.next().unwrap().is_err());
    assert!(results.next().is_none());
    let mut no_matches = db
        .search(
            "__no_such_symbol__",
            SearchOptions {
                cancel: Some(&cancel),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(no_matches.next().unwrap().is_err());
    assert!(no_matches.next().is_none());
}

#[test]
fn fuzzy_search_ranks_before_paging_and_rejects_prefix_option() {
    let (code, result, _) = cli(&["search", "CreateFileW", "--limit", "1000"]);
    assert_eq!(code, 0);
    let matches = result["result"]["matches"].as_array().unwrap();
    assert_eq!(matches[0]["name"], "CreateFileW");
    let (code, result, _) = cli(&["search", "CrtFlW", "--limit", "1000"]);
    assert_eq!(code, 0);
    assert!(
        result["result"]["matches"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["name"] == "CreateFileW")
    );
    assert_eq!(cli(&["search", "file", "--prefix"]).0, 2);
    let (_, exact, _) = cli(&["search", "createfilew", "--name"]);
    assert!(
        exact["result"]["matches"]
            .as_array()
            .unwrap()
            .iter()
            .all(|m| m["name"]
                .as_str()
                .unwrap()
                .eq_ignore_ascii_case("CreateFileW"))
    );
}

#[test]
fn fuzzy_scope_is_selected_by_the_query_and_exact_selectors_remain_independent() {
    use winlex_core::{MatchField, SearchStep};
    let db = catalog();
    let collect = |query: &str| {
        db.search(query, SearchOptions::default())
            .unwrap()
            .collect::<winlex_core::Result<Vec<_>>>()
            .unwrap()
    };
    assert!(collect("WindowsWin32").is_empty());
    let short = collect("CrtFlW");
    assert!(short.iter().any(|m| m.name == "CreateFileW"));
    assert!(
        short
            .iter()
            .all(|m| m.quality.field() == Some(MatchField::Name))
    );
    let qualified = collect("Storage.FileSystem.CrtFlW");
    assert!(qualified.iter().any(|m| m.name == "CreateFileW"));
    assert!(
        qualified
            .iter()
            .all(|m| m.quality.field() == Some(MatchField::FullName))
    );
    let mut search = db.search("CrtFlW", SearchOptions::default()).unwrap();
    let mut stepped = vec![];
    let mut skipped = 0;
    loop {
        match search.step().unwrap() {
            SearchStep::Match(m) => stepped.push(m.id),
            SearchStep::Skipped => skipped += 1,
            SearchStep::Done => break,
        }
    }
    assert!(skipped > 0);
    assert_eq!(stepped, short.iter().map(|m| m.id).collect::<Vec<_>>());
    assert!(matches!(search.step().unwrap(), SearchStep::Done));
    assert!(search.next().is_none());
    let mut mixed = db.search("CrtFlW", SearchOptions::default()).unwrap();
    let first = loop {
        match mixed.step().unwrap() {
            SearchStep::Match(m) => break m.id,
            SearchStep::Skipped => {}
            SearchStep::Done => panic!("missing fuzzy match"),
        }
    };
    assert_eq!(first, short[0].id);
    assert_eq!(
        mixed.map(|m| m.unwrap().id).collect::<Vec<_>>(),
        stepped[1..]
    );
    let cancel = AtomicBool::new(false);
    let mut cancelled = db
        .search(
            "CrtFlW",
            SearchOptions {
                cancel: Some(&cancel),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(!matches!(cancelled.step().unwrap(), SearchStep::Done));
    cancel.store(true, std::sync::atomic::Ordering::Relaxed);
    assert!(
        cancelled
            .step()
            .unwrap_err()
            .to_string()
            .contains("cancelled")
    );
    assert!(matches!(cancelled.step().unwrap(), SearchStep::Done));
    assert!(cancelled.next().is_none());
    let name = "Windows.Win32.Storage.FileSystem.Apis.CreateFileW";
    let id = winlex_cli::select(db, name, Some(Architecture::X64), false).unwrap();
    assert_eq!(db.entity(id).unwrap().name, "CreateFileW");
    let (code, exact, _) = cli(&["search", name, "--name"]);
    assert_eq!(code, 0);
    assert!(
        exact["result"]["matches"]
            .as_array()
            .unwrap()
            .iter()
            .all(|m| m["full_name"] == name)
    );
}

#[cfg(windows)]
#[test]
fn source_paths_preserve_native_utf16_without_changing_json() {
    use std::os::windows::ffi::OsStringExt;
    let directory = std::env::temp_dir().join(format!("winlex-native-path-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join(OsString::from_wide(&[
        0xd800, 0x2e, 0x77, 0x69, 0x6e, 0x6d, 0x64,
    ]));
    std::fs::copy(input(), &path).unwrap();
    let db = Catalog::open(&path).unwrap();
    assert_eq!(db.sources()[0].path, path);
    let value = serde_json::to_value(&db.sources()[0]).unwrap();
    assert!(value.get("native_path").is_none());
    assert!(value["path"].as_str().unwrap().contains('\u{fffd}'));
    let (mut out, mut err) = (vec![], vec![]);
    assert_eq!(
        winlex_cli::run(
            [
                path.clone().into_os_string(),
                OsString::from("summary"),
                OsString::from("--json"),
            ],
            &mut out,
            &mut err,
        ),
        0
    );
    assert!(err.is_empty());
    assert!(serde_json::from_slice::<Value>(&out).is_ok());
    std::fs::remove_file(path).unwrap();
    std::fs::remove_dir(directory).unwrap();
}
fn cli(args: &[&str]) -> (u8, Value, String) {
    let mut argv = vec![input().into_os_string(), OsString::from("--json")];
    argv.extend(args.iter().map(OsString::from));
    let (mut out, mut err) = (vec![], vec![]);
    let code = winlex_cli::run(argv, &mut out, &mut err);
    (
        code,
        serde_json::from_slice(&out).expect("exactly one JSON response"),
        String::from_utf8(err).unwrap(),
    )
}

#[test]
fn cli_errors_are_machine_readable() {
    for (args, code) in [
        (vec!["layout", "OPENFILENAMEW"], 2),
        (vec!["search", "x", "--limit", "1001"], 2),
        (vec!["show", "0x02ffffff"], 3),
        (vec!["show", "OPENFILENAMEW"], 3),
        (vec!["search", "invalid", "--guid"], 3),
    ] {
        let (c, v, e) = cli(&args);
        assert_eq!(c, code);
        assert_eq!(v["ok"], false);
        assert!(e.is_empty());
        assert!(v["error"]["code"].is_string());
    }
}

#[test]
fn architecture_ambiguity_and_pagination_are_preserved() {
    let (c, v, _) = cli(&["search", "OPENFILENAMEW", "--name", "--limit", "1"]);
    assert_eq!(c, 0);
    assert_eq!(v["result"]["total"], 3);
    assert_eq!(v["result"]["matches"][0]["kind"], "type_reference");
    assert_eq!(v["result"]["matches"].as_array().unwrap().len(), 1);
    let (_, next, _) = cli(&[
        "search",
        "OPENFILENAMEW",
        "--name",
        "--limit",
        "1",
        "--offset",
        "1",
    ]);
    assert_ne!(
        v["result"]["matches"][0]["id"],
        next["result"]["matches"][0]["id"]
    );
    let (_, v, _) = cli(&["search", "OPENFILENAMEW", "--name", "--arch", "x64"]);
    assert_eq!(v["result"]["total"], 2);
}

#[test]
fn declaration_to_callback_reference_is_navigable() {
    let (code, response, _) = cli(&["show", "OPENFILENAMEW", "--arch", "x64"]);
    assert_eq!(code, 0);
    let d = &response["result"];
    let fields = d["fields"].as_array().unwrap();
    assert_eq!(fields.len(), 23);
    let f = fields
        .iter()
        .find(|f| f["entity"]["name"] == "lpfnHook")
        .unwrap();
    assert_eq!(f["type"]["name"], "LPOFNHOOKPROC");
    let (code, r, _) = cli(&[
        "show",
        &entity_id(&f["type"]["reference_id"]).to_string(),
        "--arch",
        "x64",
    ]);
    assert_eq!(code, 0);
    let (code, response, _) = cli(&[
        "show",
        &entity_id(&r["result"]["resolution"]["target_id"]).to_string(),
        "--arch",
        "x64",
    ]);
    assert_eq!(code, 0);
    let callback = &response["result"];
    let invoke = callback["methods"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["entity"]["name"] == "Invoke")
        .unwrap();
    assert_eq!(invoke["return_type"]["name"], "USize");
    assert_eq!(invoke["parameters"].as_array().unwrap().len(), 4);
    assert!(
        callback["attributes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a["name"] == "UnmanagedFunctionPointerAttribute")
    );
}

#[test]
fn api_has_import_and_parameter_semantics() {
    let (code, response, _) = cli(&["show", "CreateFileW", "--arch", "x64"]);
    assert_eq!(code, 0);
    let d = &response["result"];
    assert_eq!(d["entity"]["kind"], "method");
    assert_eq!(
        d["import"]["dll"].as_str().unwrap().to_lowercase(),
        "kernel32.dll"
    );
    assert_eq!(d["return_type"]["name"], "HANDLE");
    assert_eq!(d["parameters"].as_array().unwrap().len(), 7);
    assert_eq!(d["parameters"][0]["name"], "lpFileName");
    assert_eq!(d["parameters"][0]["sequence"], 1);
}

#[test]
fn values_guids_and_source_identity() {
    let (_, decimal, _) = cli(&["search", "2147483648", "--value"]);
    let (_, hex, _) = cli(&["search", "0x80000000", "--value"]);
    assert_eq!(decimal["result"], hex["result"]);
    assert_eq!(decimal["schema"], "winlex-cli-6");
    assert_eq!(decimal["sources"][0]["id"], 0);
    let (code, response, _) = cli(&["show", "GENERIC_READ", "--arch", "x64"]);
    assert_eq!(code, 0);
    let d = &response["result"];
    assert_eq!(d["value"]["decimal"], "2147483648");
    assert_eq!(d["value"]["hex"], "0x80000000");
    let (_, g, _) = cli(&["search", "{0000000C-0000-0000-C000-000000000046}", "--guid"]);
    assert!(
        g["result"]["matches"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["name"] == "IStream")
    );
}

#[test]
fn calculations_use_selected_architecture() {
    let db = catalog();
    let layout = |name, arch| {
        db.layout(
            winlex_cli::select(db, name, Some(arch), true).unwrap(),
            arch,
        )
        .unwrap()
    };
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
        .vtable(
            winlex_cli::select(db, "IStream", Some(Architecture::X64), true).unwrap(),
            Architecture::X64,
        )
        .unwrap();
    assert_eq!(vt.slots[5].method, "Seek");
    assert_eq!(vt.slots[13].method, "Clone");
}

#[test]
fn reverse_uses_and_cancellation() {
    let db = catalog();
    let r = db
        .references(
            winlex_cli::select(db, "LPOFNHOOKPROC", Some(Architecture::X64), true).unwrap(),
            Architecture::X64,
            None,
        )
        .unwrap()
        .collect::<winlex_core::Result<Vec<_>>>()
        .unwrap();
    assert!(
        r.iter()
            .any(|e| e.full_name.ends_with("OPENFILENAMEW.lpfnHook"))
    );
    let stop = AtomicBool::new(true);
    let e = db
        .references(
            winlex_cli::select(db, "LPOFNHOOKPROC", Some(Architecture::X64), true).unwrap(),
            Architecture::X64,
            Some(&stop),
        )
        .unwrap()
        .next()
        .unwrap()
        .unwrap_err();
    assert!(e.to_string().contains("cancelled"));
}

#[test]
fn immutable_catalog_can_be_shared_by_workers() {
    fn send_sync<T: Send + Sync>() {}
    send_sync::<Catalog>();
    let db = Arc::new(Catalog::open(input()).unwrap());
    let jobs = [Architecture::X86, Architecture::X64].map(|a| {
        let db = db.clone();
        std::thread::spawn(move || {
            db.layout(
                winlex_cli::select(&db, "OPENFILENAMEW", Some(a), true).unwrap(),
                a,
            )
            .unwrap()
            .size_bits
        })
    });
    let [x86, x64] = jobs;
    assert_eq!(x86.join().unwrap(), 704);
    assert_eq!(x64.join().unwrap(), 1216);
}

#[test]
fn qualified_name_and_token_select_the_same_declaration() {
    let db = catalog();
    let id = winlex_cli::select(
        db,
        "Windows.Win32.System.IO.OVERLAPPED",
        Some(Architecture::X64),
        true,
    )
    .unwrap();
    assert_eq!(
        winlex_cli::select(db, &id.to_string(), Some(Architecture::X64), true).unwrap(),
        id
    );
    let mut p = db
        .search(
            "OPENFILE",
            SearchOptions {
                mode: SearchMode::Combined,
                ..Default::default()
            },
        )
        .unwrap();
    assert!(p.next().unwrap().is_ok());
}

#[test]
fn duplicate_inputs_and_numeric_source_selectors() {
    let path = input();
    let path = path.to_str().unwrap();
    let (code, result, _) = cli(&[path, "search", "CreateFileW", "--name"]);
    assert_eq!(code, 0);
    assert_eq!(result["sources"].as_array().unwrap().len(), 2);
    let matches = result["result"]["matches"].as_array().unwrap();
    assert_eq!(matches.len(), 2);
    for (index, entity) in matches.iter().enumerate() {
        let id = entity_id(&entity["id"]);
        assert_eq!(id.source_id, index);
        let (code, shown, _) = cli(&[path, "show", &id.to_string(), "--arch", "x64"]);
        assert_eq!(code, 0);
        assert_eq!(shown["result"]["entity"]["id"], entity["id"]);
    }
    let (code, filtered, _) = cli(&[path, "--source", "1", "search", "CreateFileW", "--name"]);
    assert_eq!(code, 0);
    assert_eq!(filtered["result"]["total"], 1);
    assert_eq!(filtered["result"]["matches"][0]["id"]["source_id"], 1);
    assert_eq!(cli(&[path, "show", "CreateFileW", "--arch", "x64"]).0, 3);
    assert_eq!(cli(&["--source", "1", "search", "CreateFileW"]).0, 3);
}

#[test]
fn exact_selection_does_not_fall_back_to_case_insensitive_search() {
    let (code, result, _) = cli(&["show", "createfilew", "--arch", "x64"]);
    assert_eq!(code, 3);
    assert!(
        result["error"]["message"]
            .as_str()
            .unwrap()
            .contains("found 0")
    );
    let (code, result, _) = cli(&["search", "createfilew", "--name"]);
    assert_eq!(code, 0);
    assert_eq!(result["result"]["total"], 1);
    let id = entity_id(&result["result"]["matches"][0]["id"]);
    assert_eq!(cli(&["show", &id.to_string(), "--arch", "x64"]).0, 0);
    let (code, empty, _) = cli(&["search", "file", "--limit", "0"]);
    assert_eq!(code, 0);
    assert!(empty["result"]["total"].as_u64().unwrap() > 100);
    assert!(empty["result"]["matches"].as_array().unwrap().is_empty());
}

#[test]
fn text_output_and_input_error() {
    let (code, response, _) = cli(&["vtable", "IStream", "--arch", "x64"]);
    assert_eq!(code, 0);
    assert_eq!(response["result"]["slots"][5]["byte_offset"], 40);
    assert_eq!(response["result"]["architecture"], "x64");
    assert!(response["result"].get("arch").is_none());
    assert!(response["result"].get("windows_runtime").is_none());
    assert_eq!(response["result"]["slots"][5]["implicit"], false);
    let (code, response, _) = cli(&["layout", "DCB", "--arch", "x86"]);
    assert_eq!(code, 0);
    let layout = &response["result"];
    assert_eq!(layout["architecture"], "x86");
    assert!(layout.get("arch").is_none());
    let field = layout["fields"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["name"] == "_bitfield")
        .unwrap();
    assert!(field.get("fields").is_none());
    let bit = field["bitfields"]
        .as_array()
        .unwrap()
        .iter()
        .find(|b| b["name"] == "fDtrControl")
        .unwrap();
    assert_eq!(bit["relative_offset_bits"], 4);
    assert_eq!(bit["offset_bits"], 68);
    for (args, expected) in [
        (
            vec!["vtable", "IStream", "--arch", "x64"],
            vec!["IStream.Seek", "byte-offset=40"],
        ),
        (
            vec!["layout", "DCB", "--arch", "x64"],
            vec![
                "size=224 bits",
                "bitfield fDtrControl: offset=68 bits, width=2 bits",
            ],
        ),
        (
            vec!["show", "CreateFileW", "--arch", "x64"],
            vec!["CreateFileW", "KERNEL32.dll!CreateFileW", "lpFileName", "@"],
        ),
        (
            vec!["show", "GENERIC_READ", "--arch", "x64"],
            vec!["2147483648", "0x80000000"],
        ),
        (
            vec!["show", "OVERLAPPED", "--arch", "x64"],
            vec!["layout: sequential", "hEvent", "type:"],
        ),
        (
            vec!["search", "CreateFileW", "--name"],
            vec!["1 match(es)", "CreateFileW"],
        ),
        (
            vec!["references", "LPOFNHOOKPROC", "--arch", "x64"],
            vec!["References to", "OPENFILENAMEW.lpfnHook"],
        ),
        (
            vec!["summary"],
            vec!["type rows", "Windows.Win32.winmd", "assembly:"],
        ),
    ] {
        let args = [input().into_os_string()]
            .into_iter()
            .chain(args.into_iter().map(OsString::from));
        let (mut out, mut err) = (vec![], vec![]);
        assert_eq!(
            winlex_cli::run(args, &mut out, &mut err),
            0,
            "{}",
            String::from_utf8_lossy(&err)
        );
        let text = String::from_utf8(out).unwrap();
        for expected in expected {
            assert!(text.contains(expected), "missing {expected:?} in {text}");
        }
        assert!(err.is_empty());
    }
    let args = ["/does-not-exist.winmd", "--json", "summary"].map(OsString::from);
    let (mut out, mut err) = (vec![], vec![]);
    assert_eq!(winlex_cli::run(args, &mut out, &mut err), 3);
    let e: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(e["error"]["code"], "input_error");
}
