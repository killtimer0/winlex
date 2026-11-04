use std::path::PathBuf;
use windows_metadata::{HasAttributes, MethodAttributes, Value, reader::Index};

fn root(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("verification")
        .join(name);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn declaration_roundtrip_preserves_abi_and_varargs_without_imports() {
    let dir = root("declaration-roundtrip");
    let input = dir.join("input.rdl");
    std::fs::write(
        &input,
        r#"
        #[win32] mod Probe {
            #[declaration] extern "C" fn print(format: *const i8, ...) -> i32;
            #[declaration] extern "system" fn system_fn(value: i32);
            #[declaration] extern "fastcall" fn fast_fn(value: i32);
        }
    "#,
    )
    .unwrap();
    let metadata = dir.join("first.winmd");
    windows_rdl::reader()
        .input(&input)
        .input(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("declarations.rdl"))
        .output(&metadata)
        .write()
        .unwrap();
    let rdl = dir.join("roundtrip.rdl");
    windows_rdl::writer()
        .input(&metadata)
        .output(&rdl)
        .write()
        .unwrap();
    let second = dir.join("second.winmd");
    windows_rdl::reader()
        .input(&rdl)
        .output(&second)
        .write()
        .unwrap();
    for path in [&metadata, &second] {
        let index = Index::read(path).unwrap();
        let apis = index.expect("Probe", "Apis");
        for (name, abi, variadic) in [
            ("print", "C", true),
            ("system_fn", "system", false),
            ("fast_fn", "fastcall", false),
        ] {
            let method = apis.methods().find(|m| m.name() == name).unwrap();
            assert!(method.impl_map().is_none());
            assert!(!method.flags().contains(MethodAttributes::PInvokeImpl));
            assert_eq!(
                method.signature(&[]).flags.0 & 0xf,
                if variadic { 5 } else { 0 }
            );
            let attr = method
                .attributes()
                .find(|a| {
                    a.namespace() == "Winlex.Metadata"
                        && a.name() == "NativeCallingConventionAttribute"
                })
                .unwrap();
            assert_eq!(attr.value(), vec![(String::new(), Value::Utf8(abi.into()))]);
        }
    }
}

#[test]
fn declarations_reject_library_and_import_mode_still_works() {
    let dir = root("import-contract");
    let input = dir.join("input.rdl");
    std::fs::write(
        &input,
        r#"#[win32] mod Probe {
        #[declaration] #[library("example")] extern "C" fn invalid();
    }"#,
    )
    .unwrap();
    assert!(
        windows_rdl::reader()
            .input(&input)
            .output(dir.join("invalid.winmd"))
            .write()
            .is_err()
    );
    std::fs::write(
        &input,
        r#"#[win32] mod Probe {
        #[library("example")] extern "C" fn original(value: i32);
    }"#,
    )
    .unwrap();
    let path = dir.join("import.winmd");
    windows_rdl::reader()
        .input(&input)
        .output(&path)
        .write()
        .unwrap();
    let index = Index::read(&path).unwrap();
    let method = index.expect("Probe", "Apis").methods().next().unwrap();
    assert!(method.flags().contains(MethodAttributes::PInvokeImpl));
    assert_eq!(method.impl_map().unwrap().import_scope().name(), "example");
}

#[test]
fn nested_record_references_keep_parent_scopes_through_merge_and_roundtrip() {
    use windows_metadata::{
        AsRow, Type,
        reader::{ResolutionScope, TypeDef, TypeDefOrRef},
    };

    fn check(index: &Index, record: TypeDef<'_>, path: &str) {
        for child in index.nested(record) {
            let child_path = format!("{path}/{}", child.name());
            let field = record.fields().find(|field| {
                matches!(field.ty(), Type::ValueName(name) if name.namespace == "Probe" && name.name == child_path)
            }).unwrap_or_else(|| panic!("missing qualified nested reference: {child_path}"));
            let mut blob = field.blob(2);
            assert_eq!(blob.read_u8(), 0x06);
            assert_eq!(blob.read_u8(), 0x11);
            let TypeDefOrRef::TypeRef(reference) = blob.decode() else {
                panic!("expected nested TypeRef");
            };
            assert_eq!(reference.name(), child.name());
            let ResolutionScope::TypeRef(parent) = reference.scope() else {
                panic!("nested reference must be scoped to its enclosing type");
            };
            assert_eq!(parent.name(), record.name());
            assert_eq!(TypeDefOrRef::TypeRef(parent).type_name().name, path);
            if child
                .flags()
                .contains(windows_metadata::TypeAttributes::ExplicitLayout)
            {
                assert!(
                    child
                        .fields()
                        .all(|field| field.explicit_offset() == Some(0))
                );
            }
            check(index, child, &child_path);
        }
    }

    let dir = root("nested-reference-contract");
    let rdl = dir.join("input.rdl");
    std::fs::write(
        &rdl,
        r#"#[win32] mod Probe {
        struct Record {
            data: union {
                integer: i32,
                nested: struct {
                    value: i64,
                    data: union { byte: u8, pointer: *mut u8 },
                },
            },
            tail: u32,
        }
    }"#,
    )
    .unwrap();
    let original = dir.join("original.winmd");
    windows_rdl::reader()
        .input(&rdl)
        .output(&original)
        .write()
        .unwrap();
    let combined = dir.join("combined.winmd");
    windows_metadata::merge::Merger::new()
        .arch_input(&original, 1)
        .arch_input(&original, 2)
        .arch_input(&original, 4)
        .output(&combined)
        .merge()
        .unwrap();
    let rdl = dir.join("roundtrip.rdl");
    windows_rdl::writer()
        .input(&combined)
        .output(&rdl)
        .write()
        .unwrap();
    let roundtrip = dir.join("roundtrip.winmd");
    windows_rdl::reader()
        .input(&rdl)
        .output(&roundtrip)
        .write()
        .unwrap();
    for path in [&original, &combined, &roundtrip] {
        let index = Index::read(path).unwrap();
        let record = index.expect("Probe", "Record");
        assert_eq!(index.nested(record).count(), 1);
        check(&index, record, "Record");
    }
}

#[test]
fn generated_crt_contains_only_declarations() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("out/UCRT.winmd");
    let index = Index::read(path).unwrap();
    let apis = index.expect("Ucrt.CRuntime", "Apis");
    let mut count = 0;
    for method in apis.methods() {
        assert!(method.impl_map().is_none(), "{}", method.name());
        assert!(!method.flags().contains(MethodAttributes::PInvokeImpl));
        assert_eq!(method.impl_flags().0, 0x15);
        assert!(
            method
                .attributes()
                .any(|a| a.namespace() == "Winlex.Metadata"
                    && a.name() == "NativeCallingConventionAttribute")
        );
        count += 1;
    }
    assert!(count > 500);
}

#[test]
fn crt_query_contracts_survive_all_architecture_roundtrips() {
    use windows_metadata::{ParamAttributes, Type};
    for arch in ["x86", "x64", "arm64"] {
        for filename in ["UCRT.winmd", "UCRT.roundtrip.winmd"] {
            let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("out")
                .join(arch)
                .join(filename);
            let index = Index::read(path).unwrap();
            let apis = index.expect("Ucrt.CRuntime", "Apis");
            let method = |name: &str| apis.methods().find(|m| m.name() == name).unwrap();
            for name in [
                "_beginthreadex",
                "_stat32",
                "_stat32i64",
                "_stat64",
                "_stat64i32",
                "_MallocaComputeSize",
            ] {
                let item = method(name);
                assert!(item.impl_map().is_none());
                assert_eq!(item.impl_flags().0, 0x15);
            }
            for name in [
                "va_list",
                "size_t",
                "ptrdiff_t",
                "intptr_t",
                "uintptr_t",
                "rsize_t",
                "float_t",
                "double_t",
            ] {
                assert!(
                    index.contains("Ucrt.CRuntime", name),
                    "{arch}/{filename}: {name}"
                );
            }
            let vprintf = method("vprintf");
            assert!(
                matches!(&vprintf.signature(&[]).types[1], Type::ValueName(name) if name.name == "va_list")
            );
            let malloc = method("malloc");
            assert!(
                matches!(&malloc.signature(&[]).types[0], Type::ValueName(name) if name.name == "size_t")
            );
            for item in [vprintf, malloc] {
                for param in item.params() {
                    assert!(!param.flags().contains(ParamAttributes::Out));
                    assert!(!param.flags().contains(ParamAttributes::In));
                }
            }
            let callback = index.expect("Ucrt.CRuntime", "_beginthreadex_proc_type");
            let abi = callback
                .attributes()
                .find(|a| a.name() == "UnmanagedFunctionPointerAttribute")
                .unwrap();
            assert!(
                matches!(&abi.value()[0].1, Value::EnumValue(_, value) if **value == Value::I32(1))
            );
            let invoke = callback.methods().find(|m| m.name() == "Invoke").unwrap();
            assert_eq!(invoke.params().next().unwrap().flags().0, 0);
            for name in ["_stat32", "_stat32i64", "_stat64", "_stat64i32"] {
                assert!(index.expect("Ucrt.CRuntime", name).fields().count() > 5);
            }
        }
    }
}

#[test]
fn emitted_inventory_and_roundtrip_signature_contract() {
    use std::collections::BTreeMap;
    for arch in ["x86", "x64", "arm64"] {
        let out = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("out")
            .join(arch);
        let mut original = BTreeMap::new();
        let mut inventory = String::from("kind\towner\tname\tparameter_count\tvariadic\tfields\n");
        for filename in ["UCRT.winmd", "UCRT.roundtrip.winmd"] {
            let index = Index::read(out.join(filename)).unwrap();
            let mut snapshot = BTreeMap::new();
            for ty in index.types().filter(|t| t.namespace() == "Ucrt.CRuntime") {
                let mut fields: Vec<_> = ty
                    .fields()
                    .map(|f| (f.name().to_string(), format!("{:?}", f.ty())))
                    .collect();
                if ty.name() == "Apis" {
                    fields.sort();
                }
                if filename == "UCRT.winmd" {
                    inventory.push_str(&format!(
                        "type\t\t{}\t\t\t{}\n",
                        ty.name(),
                        fields
                            .iter()
                            .map(|f| f.0.as_str())
                            .collect::<Vec<_>>()
                            .join(",")
                    ));
                }
                snapshot.insert(format!("type:{}", ty.name()), format!("{fields:?}"));
                for method in ty.methods() {
                    let sig = method.signature(&[]);
                    let params: Vec<_> = method
                        .params()
                        .map(|p| (p.sequence(), p.name().to_string(), p.flags().0))
                        .collect();
                    snapshot.insert(
                        format!("method:{}.{}", ty.name(), method.name()),
                        format!(
                            "{:?}|{:?}|{}|{params:?}",
                            sig.return_type, sig.types, sig.flags.0
                        ),
                    );
                    if filename == "UCRT.winmd" {
                        inventory.push_str(&format!(
                            "method\t{}\t{}\t{}\t{}\t\n",
                            ty.name(),
                            method.name(),
                            sig.types.len(),
                            (sig.flags.0 & 15) == 5
                        ));
                    }
                }
            }
            if filename == "UCRT.winmd" {
                original = snapshot;
            } else {
                assert_eq!(original.len(), snapshot.len());
                for (key, value) in &original {
                    assert_eq!(Some(value), snapshot.get(key), "{arch}: {key}");
                }
            }
        }
        std::fs::write(out.join("winmd-inventory.tsv"), inventory).unwrap();
    }
}
