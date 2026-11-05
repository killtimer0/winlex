//! Integration tests for the CLI adapter and query library.
use serde_json::Value;
use std::{ffi::OsString, path::PathBuf, sync::OnceLock};
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

fn invoke(args: impl IntoIterator<Item = OsString>) -> (u8, String, String) {
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let code = winlex_cli::run(args, &mut out, &mut err);
    (
        code,
        String::from_utf8(out).unwrap(),
        String::from_utf8(err).unwrap(),
    )
}

#[test]
fn search_mode_flags_are_exclusive_and_only_apply_to_search() {
    for arguments in [
        vec!["search", "1", "--name", "--value"],
        vec!["search", "1", "--value", "--guid"],
        vec!["search", "1", "--guid", "--name"],
        vec!["search", "1", "--name", "--name"],
        vec!["search", "--where", "value = 5", "--value"],
        vec!["search", "--name", "--where", "value = 5"],
        vec!["show", "--where", "value = 5"],
        vec!["show", "CreateFileW", "--name"],
        vec!["summary", "--value"],
        vec!["search", "CreateFileW", "--exact"],
        vec!["value", "1"],
        vec!["guid", "1234"],
    ] {
        let (code, result, _) = cli(&arguments);
        assert_eq!(code, 2, "{arguments:?}");
        assert_eq!(result["ok"], false);
    }
    for (query, flag) in [
        ("08", "--value"),
        ("Infinity", "--value"),
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
fn conditions_find_real_constants_and_report_input_spans() {
    let (code, result, _) = cli(&[
        "search",
        "--where",
        r#"name contains "access" and value = 5"#,
    ]);
    assert_eq!(code, 0, "{result}");
    let matches = result["result"]["matches"].as_array().unwrap();
    assert!(matches.iter().any(|r| r["name"] == "ERROR_ACCESS_DENIED"));
    for row in matches {
        assert!(
            row["name"]
                .as_str()
                .unwrap()
                .to_lowercase()
                .contains("access")
        );
        let evidence = row["evidence"].as_array().unwrap();
        assert!(evidence.iter().any(|e| e["field"] == "name"));
        assert!(evidence.iter().any(|e| e["field"] == "value"));
    }
    let (code, error, _) = cli(&["search", "--where", "value < nan"]);
    assert_eq!(code, 3);
    assert_eq!(error["error"]["span"]["start"], 0);
    assert_eq!(error["error"]["incomplete"], false);
    let (code, error, _) = cli(&["search", "--where", "value ="]);
    assert_eq!(code, 3);
    assert_eq!(error["error"]["incomplete"], true);
    for query in ["nan", "inf", "+inf", "-inf"] {
        assert_eq!(cli(&["search", query, "--value"]).0, 0);
    }
}

#[test]
fn guid_fragments_bytes_and_numeric_radices_share_the_cli_search_contract() {
    let (_, expected, _) = cli(&["search", "2147483648", "--value", "--limit", "1000"]);
    assert_eq!(expected["sources"][0]["id"], 0);
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
fn queries_require_explicit_inputs_in_text_and_json_modes() {
    for args in [vec!["summary"], vec!["search", "CreateFileW", "--json"]] {
        let (code, out, err) = invoke(args.iter().map(OsString::from));
        assert_eq!(code, 2);
        if args.contains(&"--json") {
            let response: Value = serde_json::from_str(&out).unwrap();
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
            assert!(err.contains("input file"));
        }
    }
}

#[test]
fn fuzzy_search_accepts_abbreviations_and_rejects_prefix_option() {
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
}

#[test]
fn fuzzy_scope_is_selected_by_the_query_and_exact_selectors_remain_independent() {
    let db = catalog();
    let (code, qualified, _) = cli(&["search", "Storage.FileSystem.CrtFlW"]);
    assert_eq!(code, 0);
    assert!(
        qualified["result"]["matches"]
            .as_array()
            .unwrap()
            .iter()
            .any(|hit| hit["name"] == "CreateFileW")
    );
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
    let (code, out, err) = invoke([
        path.clone().into_os_string(),
        "summary".into(),
        "--json".into(),
    ]);
    assert_eq!(code, 0);
    assert!(err.is_empty());
    assert!(serde_json::from_str::<Value>(&out).is_ok());
    std::fs::remove_file(path).unwrap();
    std::fs::remove_dir(directory).unwrap();
}
fn cli(args: &[&str]) -> (u8, Value, String) {
    let mut argv = vec![input().into_os_string(), OsString::from("--json")];
    argv.extend(args.iter().map(OsString::from));
    let (code, out, err) = invoke(argv);
    let response: Value = serde_json::from_str(&out).expect("exactly one JSON response");
    assert_eq!(response["schema"], "winlex-cli-1", "{args:?}");
    (code, response, err)
}

#[test]
fn cli_errors_are_machine_readable() {
    for (args, code) in [
        (vec!["summary", "--unknown"], 2),
        (vec!["layout", "OPENFILENAMEW"], 2),
        (vec!["vtable", "IStream"], 2),
        (vec!["references", "LPOFNHOOKPROC"], 2),
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
fn error_code_json_uses_the_same_schema_without_metadata() {
    for (args, expected) in [
        (vec!["error-code", "5", "--scope", "win32", "--json"], 0),
        (vec!["error-code", "invalid", "--json"], 2),
        (
            vec![
                "error-code",
                "5",
                "--module",
                "winlex-missing-message-module-for-schema-test.dll",
                "--json",
            ],
            3,
        ),
    ] {
        let (code, out, err) = invoke(args.iter().map(OsString::from));
        assert_eq!(code, expected, "{args:?}: {out}");
        assert!(err.is_empty());
        let response: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(response["schema"], "winlex-cli-1");
        assert_eq!(response["ok"], expected == 0);
        if expected == 2 {
            assert_eq!(response["error"]["code"], "invalid_arguments");
        } else {
            assert_eq!(response["command"], "error-code");
            assert!(response["result"].is_object());
        }
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
fn constant_values_include_decimal_and_hex() {
    let (code, response, _) = cli(&["show", "GENERIC_READ", "--arch", "x64"]);
    assert_eq!(code, 0);
    let d = &response["result"];
    assert_eq!(d["value"]["decimal"], "2147483648");
    assert_eq!(d["value"]["hex"], "0x80000000");
}

#[test]
fn reverse_uses_and_cancellation() {
    use std::sync::atomic::AtomicBool;
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
fn cli_search_options_preserve_core_modes_architectures_and_pagination() {
    for (mode, flag, query, architecture) in [
        (SearchMode::Combined, None, "createfile", None),
        (
            SearchMode::Name,
            Some("--name"),
            "createfile",
            Some(Architecture::X86),
        ),
        (SearchMode::Value, Some("--value"), "5", None),
        (SearchMode::Guid, Some("--guid"), "0000000c", None),
        (
            SearchMode::Condition,
            Some("--where"),
            "name contains \"file\"",
            Some(Architecture::X64),
        ),
    ] {
        let mut args = vec!["search"];
        if mode == SearchMode::Condition {
            args.push(flag.unwrap());
        }
        args.push(query);
        if mode != SearchMode::Condition {
            args.extend(flag);
        }
        args.extend(["--source", "0", "--offset", "1", "--limit", "3"]);
        if let Some(arch) = architecture {
            args.extend(["--arch", arch.name()]);
        }
        let (code, result, _) = cli(&args);
        assert_eq!(code, 0, "{args:?}: {result}");
        let mut expected = catalog()
            .search(
                query,
                SearchOptions {
                    source: Some(0),
                    mode,
                    architecture,
                    ..Default::default()
                },
            )
            .unwrap()
            .collect::<winlex_core::Result<Vec<_>>>()
            .unwrap();
        expected.sort_unstable_by(winlex_core::SearchMatch::compare);
        assert_eq!(result["result"]["total"], expected.len());
        assert_eq!(result["result"]["offset"], 1);
        assert_eq!(result["result"]["limit"], 3);
        let page: Vec<_> = expected.into_iter().skip(1).take(3).collect();
        assert_eq!(
            result["result"]["matches"],
            serde_json::to_value(page).unwrap()
        );
    }
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
    assert_eq!(result["result"]["matches"][0]["name"], "CreateFileW");
    let id = entity_id(&result["result"]["matches"][0]["id"]);
    assert_eq!(cli(&["show", &id.to_string(), "--arch", "x64"]).0, 0);
    let (code, empty, _) = cli(&["search", "file", "--limit", "0"]);
    assert_eq!(code, 0);
    assert!(empty["result"]["total"].as_u64().unwrap() > 100);
    assert!(empty["result"]["matches"].as_array().unwrap().is_empty());
}

#[test]
fn response_writer_failures_preserve_text_and_json_exit_codes() {
    struct Failing(std::io::ErrorKind);
    impl std::io::Write for Failing {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::new(self.0, "writer probe"))
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    use std::io::ErrorKind::{BrokenPipe, PermissionDenied};
    for (args, exit) in [
        (vec![input().into_os_string(), "summary".into()], 0),
        (
            vec![
                "error-code".into(),
                "5".into(),
                "--module".into(),
                input().join("missing-message-module.dll").into_os_string(),
            ],
            3,
        ),
    ] {
        for (json, kind, expected) in [
            (false, BrokenPipe, exit),
            (true, BrokenPipe, 4),
            (false, PermissionDenied, 4),
            (true, PermissionDenied, 4),
        ] {
            let mut args = args.clone();
            if json {
                args.push("--json".into());
            }
            let mut err = Vec::new();
            assert_eq!(
                winlex_cli::run(args, &mut Failing(kind), &mut err),
                expected
            );
            if expected == exit {
                assert!(err.is_empty());
            } else {
                assert!(
                    String::from_utf8(err)
                        .unwrap()
                        .starts_with("output error: ")
                );
            }
        }
    }
}

#[test]
fn json_commands_preserve_the_envelope_and_paged_result_shapes() {
    for args in [
        vec!["summary"],
        vec!["search", "CreateFileW", "--name"],
        vec!["show", "CreateFileW", "--arch", "x64"],
        vec!["layout", "DCB", "--arch", "x64"],
        vec!["vtable", "IStream", "--arch", "x64"],
        vec!["references", "LPOFNHOOKPROC", "--arch", "x64"],
    ] {
        let (code, response, err) = cli(&args);
        assert_eq!(code, 0, "{args:?}: {response}");
        assert!(err.is_empty());
        let object = response.as_object().unwrap();
        for field in ["schema", "ok", "sources", "command", "result"] {
            assert!(object.contains_key(field), "{args:?}: {field}");
        }
        assert_eq!(response["ok"], true);
        assert_eq!(response["command"], args[0]);
        assert_eq!(response["sources"].as_array().unwrap().len(), 1);
        if matches!(args[0], "search" | "references") {
            let result = &response["result"];
            let page = if args[0] == "references" {
                assert_eq!(result["architecture"], "x64");
                assert!(result["unresolved_type_uses"].is_u64());
                assert!(result["scope"].is_string());
                &result["page"]
            } else {
                result
            };
            assert_eq!(page["offset"], 0);
            assert_eq!(page["limit"], 50);
            assert!(page["total"].as_u64().unwrap() > 0);
            for row in page["matches"].as_array().unwrap() {
                assert!(row["id"].is_object());
                assert!(row["evidence"].is_array());
                assert!(row.get("quality").is_none());
            }
        }
    }
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
        let (code, text, err) = invoke(args);
        assert_eq!(code, 0, "{err}");
        for expected in expected {
            assert!(text.contains(expected), "missing {expected:?} in {text}");
        }
        assert!(err.is_empty());
    }
    let (code, response, _) = cli(&["show", "OVERLAPPED", "--arch", "x64"]);
    assert_eq!(code, 0);
    assert_eq!(response["result"]["layout_kind"], "sequential");
    let args = ["/does-not-exist.winmd", "--json", "summary"].map(OsString::from);
    let (code, out, _) = invoke(args);
    assert_eq!(code, 3);
    let e: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(e["error"]["code"], "input_error");
}
