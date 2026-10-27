//! Integration tests for the CLI adapter and query library.
use serde_json::Value;
use std::{
    ffi::OsString,
    path::PathBuf,
    sync::{Arc, OnceLock, atomic::AtomicBool},
};
use winlex_core::{Architecture, Catalog, SearchMode, SearchOptions};

fn input() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../inputs/Windows.Win32.winmd")
}
fn catalog() -> &'static Catalog {
    static DB: OnceLock<Catalog> = OnceLock::new();
    DB.get_or_init(|| Catalog::open(input()).expect("prepare pinned inputs/Windows.Win32.winmd"))
}

#[test]
fn streaming_search_is_complete_deterministic_and_cancellable() {
    use std::sync::atomic::Ordering;
    let db = catalog();
    let collect = |size| {
        let mut ids = Vec::new();
        let total = db
            .search_batches("file", size, None, |batch| {
                assert!(batch.len() <= size);
                ids.extend(batch.into_iter().map(|e| e.id));
                Ok(())
            })
            .unwrap();
        assert_eq!(total, ids.len());
        ids
    };
    let ids = collect(17);
    assert!(ids.len() > 100);
    assert_eq!(ids, collect(128));
    assert!(ids.windows(2).all(|p| p[0] < p[1]));
    let mut paged = Vec::new();
    loop {
        let page = db
            .search(
                "file",
                SearchOptions {
                    offset: paged.len(),
                    limit: 1000,
                    ..Default::default()
                },
            )
            .unwrap();
        paged.extend(page.matches.into_iter().map(|e| e.id));
        if paged.len() == page.total {
            break;
        }
    }
    paged.sort();
    assert_eq!(ids, paged);
    let cancel = AtomicBool::new(false);
    let mut batches = 0;
    assert!(
        db.search_batches("file", 8, Some(&cancel), |_| {
            batches += 1;
            cancel.store(true, Ordering::Relaxed);
            Ok(())
        })
        .is_err()
    );
    assert_eq!(batches, 1);
    let mut calls = 0;
    assert!(
        db.search_batches("file", 8, None, |_| {
            calls += 1;
            Err(winlex_core::Error::msg("consumer stopped"))
        })
        .is_err()
    );
    assert_eq!(calls, 1);
}

#[cfg(windows)]
#[test]
fn source_paths_preserve_native_utf16_without_changing_json() {
    use std::os::windows::ffi::OsStringExt;
    let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tmp/native-path-test");
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join(OsString::from_wide(&[
        0xd800, 0x2e, 0x77, 0x69, 0x6e, 0x6d, 0x64,
    ]));
    std::fs::copy(input(), &path).unwrap();
    let db = Catalog::open(&path).unwrap();
    assert_eq!(db.sources()[0].native_path, path);
    let value = serde_json::to_value(&db.sources()[0]).unwrap();
    assert!(value.get("native_path").is_none());
    assert!(value["path"].as_str().unwrap().contains('\u{fffd}'));
    std::fs::remove_file(path).unwrap();
}
fn cli(args: &[&str]) -> (u8, Value, String) {
    let mut argv = vec![
        OsString::from("--winmd"),
        input().into_os_string(),
        OsString::from("--json"),
    ];
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
        (vec!["guid", "invalid"], 3),
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
    let (c, v, _) = cli(&["search", "OPENFILENAMEW", "--exact", "--limit", "1"]);
    assert_eq!(c, 0);
    assert_eq!(v["result"]["total"], 2);
    assert_eq!(v["result"]["matches"].as_array().unwrap().len(), 1);
    let (_, next, _) = cli(&[
        "search",
        "OPENFILENAMEW",
        "--exact",
        "--limit",
        "1",
        "--offset",
        "1",
    ]);
    assert_ne!(
        v["result"]["matches"][0]["token"],
        next["result"]["matches"][0]["token"]
    );
    let (_, v, _) = cli(&["search", "OPENFILENAMEW", "--exact", "--arch", "x64"]);
    assert_eq!(v["result"]["total"], 1);
}

#[test]
fn declaration_to_callback_reference_is_navigable() {
    let db = catalog();
    let d = db
        .describe("OPENFILENAMEW", Some(Architecture::X64))
        .unwrap();
    let fields = d["fields"].as_array().unwrap();
    assert_eq!(fields.len(), 23);
    let f = fields
        .iter()
        .find(|f| f["entity"]["name"] == "lpfnHook")
        .unwrap();
    assert_eq!(f["type"]["name"], "LPOFNHOOKPROC");
    let r = db
        .describe(
            f["type"]["reference_token"].as_str().unwrap(),
            Some(Architecture::X64),
        )
        .unwrap();
    let callback = db
        .describe(
            r["resolution"]["target_token"].as_str().unwrap(),
            Some(Architecture::X64),
        )
        .unwrap();
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
    let d = catalog()
        .describe("CreateFileW", Some(Architecture::X64))
        .unwrap();
    assert_eq!(d["entity"]["kind"], "function");
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
    let (_, decimal, _) = cli(&["value", "2147483648"]);
    let (_, hex, _) = cli(&["value", "0x80000000"]);
    assert_eq!(decimal["result"], hex["result"]);
    assert_eq!(
        decimal["source_sha256"],
        "73e57def24d91f2ec025238100099d5333dd441cab5e843cd2fa8f0617472d56"
    );
    let d = catalog()
        .describe("GENERIC_READ", Some(Architecture::X64))
        .unwrap();
    assert_eq!(d["value"]["decimal"], "2147483648");
    assert_eq!(d["value"]["hex"], "0x80000000");
    let (_, g, _) = cli(&["guid", "{0000000C-0000-0000-C000-000000000046}"]);
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
    // Exact C spelling wins over the separate GdiPlus.Point and lowercase fields.
    assert_eq!(
        db.layout("POINT", Architecture::X64).unwrap()["size_bits"],
        64
    );
    assert!(
        db.describe("DCB", Some(Architecture::X64)).unwrap()["entity"]["full_name"]
            .as_str()
            .unwrap()
            .ends_with("Communication.DCB")
    );
    let dcb = db.layout("DCB", Architecture::X64).unwrap();
    let backing = dcb["fields"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["name"] == "_bitfield")
        .unwrap();
    let bit = backing["bitfields"]
        .as_array()
        .unwrap()
        .iter()
        .find(|b| b["name"] == "fDtrControl")
        .unwrap();
    assert_eq!(bit["offset_bits"], 68);
    assert_eq!(bit["width_bits"], 2);
    let x86 = db.layout("OPENFILENAMEW", Architecture::X86).unwrap();
    let x64 = db.layout("OPENFILENAMEW", Architecture::X64).unwrap();
    assert_eq!(x86["size_bits"], 704);
    assert_eq!(x64["size_bits"], 1216);
    let ov = db.layout("OVERLAPPED", Architecture::X64).unwrap();
    assert_eq!(ov["size_bits"], 256);
    let fields = ov["fields"].as_array().unwrap();
    let union = fields.iter().find(|f| f["name"] == "Anonymous").unwrap();
    assert_eq!(union["offset_bits"], 128);
    assert!(union["fields"].is_array());
    let vt = db.vtable("IStream", Architecture::X64).unwrap();
    assert_eq!(vt["slots"][5]["method"], "Seek");
    assert_eq!(vt["slots"][5]["byte_offset"], 40);
    assert_eq!(vt["slots"][13]["method"], "Clone");
}

#[test]
fn reverse_uses_and_cancellation() {
    let db = catalog();
    let r = db
        .references("LPOFNHOOKPROC", Architecture::X64, SearchOptions::default())
        .unwrap();
    assert!(r["page"]["matches"].as_array().unwrap().iter().any(|e| {
        e["full_name"]
            .as_str()
            .unwrap()
            .ends_with("OPENFILENAMEW.lpfnHook")
    }));
    let stop = AtomicBool::new(true);
    let e = db
        .search(
            "file",
            SearchOptions {
                cancel: Some(&stop),
                ..Default::default()
            },
        )
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
            db.layout("OPENFILENAMEW", a).unwrap()["size_bits"]
                .as_u64()
                .unwrap()
        })
    });
    let [x86, x64] = jobs;
    assert_eq!(x86.join().unwrap(), 704);
    assert_eq!(x64.join().unwrap(), 1216);
}

#[test]
fn full_name_and_nested_tokens_work() {
    let db = catalog();
    let d = db
        .describe(
            "Windows.Win32.System.IO.OVERLAPPED",
            Some(Architecture::X64),
        )
        .unwrap();
    let token = d["entity"]["token"].as_str().unwrap();
    assert_eq!(db.describe(token, Some(Architecture::X64)).unwrap(), d);
    let p = db
        .search(
            "OPENFILE",
            SearchOptions {
                mode: SearchMode::Prefix,
                limit: 10,
                ..Default::default()
            },
        )
        .unwrap();
    assert!(p.total > 0);
}

#[test]
fn text_output_and_input_error() {
    let args = [
        OsString::from("--winmd"),
        input().into_os_string(),
        OsString::from("vtable"),
        OsString::from("IStream"),
        OsString::from("--arch"),
        OsString::from("x64"),
    ];
    let (mut out, mut err) = (vec![], vec![]);
    assert_eq!(winlex_cli::run(args, &mut out, &mut err), 0);
    let text = String::from_utf8(out).unwrap();
    assert!(text.contains("IStream.Seek"));
    assert!(err.is_empty());
    let args = ["--winmd", "/does-not-exist.winmd", "--json", "summary"].map(OsString::from);
    let (mut out, mut err) = (vec![], vec![]);
    assert_eq!(winlex_cli::run(args, &mut out, &mut err), 3);
    let e: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(e["error"]["code"], "input_error");
}
