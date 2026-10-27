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
                    .contains("--winmd")
            );
            assert!(err.is_empty());
        } else {
            assert!(out.is_empty());
            assert!(String::from_utf8(err).unwrap().contains("--winmd"));
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
    assert!(ids.windows(2).all(|p| p[0] < p[1]));
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
    assert_eq!(db.sources()[0].path, path);
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
        v["result"]["matches"][0]["id"],
        next["result"]["matches"][0]["id"]
    );
    let (_, v, _) = cli(&["search", "OPENFILENAMEW", "--exact", "--arch", "x64"]);
    assert_eq!(v["result"]["total"], 1);
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
    assert_eq!(decimal["schema"], "winlex-cli-4");
    assert_eq!(decimal["sources"][0]["id"], 0);
    let (code, response, _) = cli(&["show", "GENERIC_READ", "--arch", "x64"]);
    assert_eq!(code, 0);
    let d = &response["result"];
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
                mode: SearchMode::Prefix,
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
    let (code, result, _) = cli(&["--winmd", path, "search", "CreateFileW", "--exact"]);
    assert_eq!(code, 0);
    assert_eq!(result["sources"].as_array().unwrap().len(), 2);
    let matches = result["result"]["matches"].as_array().unwrap();
    assert_eq!(matches.len(), 2);
    for (index, entity) in matches.iter().enumerate() {
        let id = entity_id(&entity["id"]);
        assert_eq!(id.source_id, index);
        let (code, shown, _) = cli(&["--winmd", path, "show", &id.to_string(), "--arch", "x64"]);
        assert_eq!(code, 0);
        assert_eq!(shown["result"]["entity"]["id"], entity["id"]);
    }
    let (code, filtered, _) = cli(&[
        "--winmd",
        path,
        "--source",
        "1",
        "search",
        "CreateFileW",
        "--exact",
    ]);
    assert_eq!(code, 0);
    assert_eq!(filtered["result"]["total"], 1);
    assert_eq!(filtered["result"]["matches"][0]["id"]["source_id"], 1);
    assert_eq!(
        cli(&["--winmd", path, "show", "CreateFileW", "--arch", "x64"]).0,
        3
    );
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
    let (code, result, _) = cli(&["search", "createfilew", "--exact"]);
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
            vec!["search", "CreateFileW", "--exact"],
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
        let args = [OsString::from("--winmd"), input().into_os_string()]
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
    let args = ["--winmd", "/does-not-exist.winmd", "--json", "summary"].map(OsString::from);
    let (mut out, mut err) = (vec![], vec![]);
    assert_eq!(winlex_cli::run(args, &mut out, &mut err), 3);
    let e: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(e["error"]["code"], "input_error");
}
