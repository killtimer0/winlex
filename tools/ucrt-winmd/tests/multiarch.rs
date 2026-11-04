use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use windows_metadata::{
    HasAttributes, Type, Value,
    reader::{File, Index, TypeDef},
};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn attrs<'a>(row: impl HasAttributes<'a>) -> Vec<String> {
    let mut values: Vec<_> = row
        .attributes()
        .filter(|a| {
            !(a.namespace() == "Windows.Win32.Metadata"
                && a.name() == "SupportedArchitectureAttribute")
        })
        .map(|a| format!("{}.{}={:?}", a.namespace(), a.name(), a.value()))
        .collect();
    values.sort();
    values
}

fn supported<'a>(row: impl HasAttributes<'a>, arch: i32) -> bool {
    row.arches() == 0 || row.arches() & arch != 0
}

fn insert(
    snapshot: &mut BTreeMap<String, String>,
    key: String,
    value: String,
    allow_identical: bool,
) {
    if let Some(previous) = snapshot.insert(key.clone(), value.clone()) {
        assert!(allow_identical, "overlapping architecture variants: {key}");
        assert_eq!(previous, value, "conflicting input definitions: {key}");
    }
}

fn add_type(
    index: &Index,
    ty: TypeDef<'_>,
    owner: &str,
    arch: i32,
    snapshot: &mut BTreeMap<String, String>,
    allow_identical: bool,
) {
    let name = format!("{owner}.{}", ty.name());
    insert(
        snapshot,
        format!("type:{name}"),
        format!(
            "{:?}|{:?}|{:?}|{:?}",
            ty.flags(),
            ty.extends().map(|t| (t.namespace(), t.name())),
            ty.class_layout()
                .map(|l| (l.packing_size(), l.class_size())),
            attrs(ty)
        ),
        allow_identical,
    );
    for (position, field) in ty.fields().filter(|f| supported(*f, arch)).enumerate() {
        // Struct field order is part of the ABI; Apis constant ordering is not.
        let order = if ty.name() == "Apis" { 0 } else { position };
        insert(
            snapshot,
            format!("field:{name}.{}", field.name()),
            format!(
                "{order}|{:?}|{:?}|{:?}|{:?}|{:?}",
                field.ty(),
                field.flags(),
                field.constant().map(|c| c.value()),
                field.explicit_offset(),
                attrs(field)
            ),
            allow_identical,
        );
    }
    for method in ty.methods().filter(|m| supported(*m, arch)) {
        assert!(method.impl_map().is_none());
        let sig = method.signature(&[]);
        let params: Vec<_> = method
            .params()
            .map(|p| (p.sequence(), p.name().to_owned(), p.flags(), attrs(p)))
            .collect();
        insert(
            snapshot,
            format!("method:{name}.{}", method.name()),
            format!(
                "{:?}|{:?}|{}|{:?}|{:?}|{:?}|{params:?}",
                sig.return_type,
                sig.types,
                sig.flags.0,
                method.flags(),
                method.impl_flags(),
                attrs(method)
            ),
            allow_identical,
        );
    }
    for nested in index.nested(ty).filter(|n| supported(*n, arch)) {
        add_type(index, nested, &name, arch, snapshot, allow_identical);
    }
}

fn snapshot(path: &Path, arch: i32) -> BTreeMap<String, String> {
    snapshot_with_duplicates(path, arch, false)
}

fn snapshot_with_duplicates(
    path: &Path,
    arch: i32,
    allow_identical: bool,
) -> BTreeMap<String, String> {
    let index = Index::new_for_architecture(vec![File::read(path).unwrap()], arch);
    let mut result = BTreeMap::new();
    for ty in index
        .types()
        .filter(|t| t.namespace() == "Ucrt.CRuntime" || t.namespace() == "Probe")
    {
        add_type(
            &index,
            ty,
            ty.namespace(),
            arch,
            &mut result,
            allow_identical,
        );
    }
    result
}

#[test]
fn combined_and_roundtrip_match_every_architecture_baseline() {
    for (name, arch) in [("x86", 1), ("x64", 2), ("arm64", 4)] {
        let path = root().join(format!("out/{name}/UCRT.winmd"));
        let baseline = snapshot(&path, arch);
        assert!(baseline.len() > 2000);
        for filename in ["UCRT.untagged.winmd", "UCRT.roundtrip.winmd"] {
            assert_eq!(
                snapshot_with_duplicates(
                    &root().join(format!("out/{name}/{filename}")),
                    arch,
                    filename == "UCRT.untagged.winmd",
                ),
                baseline,
                "{name}/{filename}"
            );
        }
        let index = Index::read(&path).unwrap();
        for ty in index.types().filter(|t| t.namespace() == "Ucrt.CRuntime") {
            assert_eq!(ty.arches(), arch, "{name}: {}", ty.name());
        }
        for filename in ["UCRT.winmd", "UCRT.roundtrip.winmd"] {
            assert_eq!(
                baseline,
                snapshot(&root().join("out").join(filename), arch),
                "{name}/{filename}"
            );
        }
        for other in [1, 2, 4].into_iter().filter(|a| *a != arch) {
            assert!(
                snapshot(&path, other).is_empty(),
                "wrong architecture visible in {name}"
            );
        }
    }
}

#[test]
fn merger_preserves_abi_direction_and_constant_variants() {
    let dir = root().join("verification/merge-contract");
    std::fs::create_dir_all(&dir).unwrap();
    let mut merger = windows_metadata::merge::Merger::new();
    for (name, arch, abi, direction, value, fixed) in [
        ("x86", 1, "C", "r#in", 4, "u32"),
        ("x64", 2, "system", "out", 8, "u64"),
        ("arm64", 4, "system", "out", 8, "u64"),
    ] {
        let rdl = dir.join(format!("{name}.rdl"));
        std::fs::write(
            &rdl,
            format!(
                r#"#[win32] mod Probe {{
            extern "{abi}" fn Callback(#[{direction}] p: *mut i32);
            #[declaration] extern "{abi}" fn function(#[{direction}] p: *mut i32);
            const WIDTH: u32 = {value};
            struct Record {{ value: u32 }}
            struct FixedWidth {{ value: {fixed} }}
            extern "C" fn FixedCallback(value: {fixed});
            #[declaration] extern "C" fn fixed_function(value: {fixed});
        }}"#
            ),
        )
        .unwrap();
        let path = dir.join(format!("{name}.winmd"));
        windows_rdl::reader()
            .input(rdl)
            .input(root().join("declarations.rdl"))
            .reference_default()
            .output(&path)
            .write()
            .unwrap();
        merger.arch_input(&path, arch);
    }
    let combined = dir.join("combined.winmd");
    merger.output(&combined).merge().unwrap();
    for (name, arch) in [("x86", 1), ("x64", 2), ("arm64", 4)] {
        assert_eq!(
            snapshot(&dir.join(format!("{name}.winmd")), arch),
            snapshot(&combined, arch),
            "{name}"
        );
    }
    let index = Index::read(&combined).unwrap();
    let records: Vec<_> = index.get("Probe", "Record").collect();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].arches(), 0);
    let callbacks: Vec<_> = index.get("Probe", "Callback").collect();
    assert_eq!(callbacks.len(), 2);
    assert_eq!(
        callbacks.iter().map(|c| c.arches()).collect::<Vec<_>>(),
        vec![1, 6]
    );
    for name in ["FixedWidth", "FixedCallback"] {
        assert_eq!(index.get("Probe", name).count(), 2, "{name}");
    }
    assert_eq!(
        index
            .expect("Probe", "Apis")
            .methods()
            .filter(|m| m.name() == "fixed_function")
            .count(),
        2
    );
}

#[test]
fn merger_preserves_explicit_offsets_and_splits_offset_only_variants() {
    use windows_metadata::{FieldAttributes, TypeAttributes, writer};

    let dir = root().join("verification/explicit-offset-contract");
    std::fs::create_dir_all(&dir).unwrap();
    let mut merger = windows_metadata::merge::Merger::new();
    for (name, arch, offset) in [("x86", 1, 4), ("x64", 2, 8), ("arm64", 4, 8)] {
        let mut file = writer::File::new(name);
        let base = file.TypeRef("System", "ValueType");
        let record = file.TypeDef(
            "Probe",
            "Explicit",
            writer::TypeDefOrRef::TypeRef(base),
            TypeAttributes::Public | TypeAttributes::ExplicitLayout | TypeAttributes::Sealed,
        );
        file.ClassLayout(record, 1, 16);
        for (field, offset) in [("first", 0), ("second", offset)] {
            let field = file.Field(field, &Type::U32, FieldAttributes::Public);
            file.FieldLayout(field, offset);
        }
        let input = dir.join(format!("{name}.winmd"));
        std::fs::write(&input, file.into_stream()).unwrap();
        let tagged = dir.join(format!("{name}.tagged.winmd"));
        windows_metadata::merge::Merger::new()
            .arch_input(&input, arch)
            .output(&tagged)
            .merge()
            .unwrap();
        assert_eq!(snapshot(&input, arch), snapshot(&tagged, arch));
        merger.arch_input(&tagged, arch);
    }
    let combined = dir.join("combined.winmd");
    merger.output(&combined).merge().unwrap();
    assert_eq!(
        Index::read(&combined)
            .unwrap()
            .get("Probe", "Explicit")
            .count(),
        2
    );
    for (name, arch) in [("x86", 1), ("x64", 2), ("arm64", 4)] {
        assert_eq!(
            snapshot(&dir.join(format!("{name}.winmd")), arch),
            snapshot(&combined, arch),
            "{name}"
        );
    }
}

#[test]
fn semantic_native_typedefs_are_single_architecture_neutral_definitions() {
    for filename in ["UCRT.winmd", "UCRT.roundtrip.winmd"] {
        let index = Index::read(root().join("out").join(filename)).unwrap();
        for (name, expected) in [
            ("size_t", Type::USize),
            ("uintptr_t", Type::USize),
            ("intptr_t", Type::ISize),
            ("ptrdiff_t", Type::ISize),
        ] {
            let types: Vec<_> = index.get("Ucrt.CRuntime", name).collect();
            assert_eq!(types.len(), 1, "{filename}/{name}");
            let ty = types[0];
            assert_eq!(ty.arches(), 0, "{filename}/{name}");
            assert!(
                ty.attributes()
                    .any(|a| a.name() == "NativeTypedefAttribute")
            );
            let fields: Vec<_> = ty.fields().collect();
            assert_eq!(fields.len(), 1);
            assert_eq!(fields[0].ty(), expected, "{filename}/{name}");
        }
        let rsize = index.expect("Ucrt.CRuntime", "rsize_t");
        assert!(
            matches!(rsize.fields().next().unwrap().ty(), Type::ValueName(name) if name.name == "size_t")
        );
        let methods: Vec<_> = index
            .expect("Ucrt.CRuntime", "Apis")
            .methods()
            .filter(|m| m.name() == "__report_gsfailure")
            .collect();
        assert_eq!(methods.len(), 2);
        for (arch, count) in [(1, 0), (6, 1)] {
            let method = methods.iter().find(|m| m.arches() == arch).unwrap();
            assert_eq!(method.signature(&[]).types.len(), count);
        }
        let checks: Vec<_> = index
            .expect("Ucrt.CRuntime", "Apis")
            .methods()
            .filter(|m| m.name() == "__security_check_cookie")
            .collect();
        assert_eq!(checks.len(), 2);
        for (arch, abi) in [(1, "fastcall"), (6, "C")] {
            let method = checks.iter().find(|m| m.arches() == arch).unwrap();
            assert!(
                method
                    .attributes()
                    .any(|a| a.name() == "NativeCallingConventionAttribute"
                        && a.value()[0].1 == Value::Utf8(abi.into()))
            );
        }
    }
}
