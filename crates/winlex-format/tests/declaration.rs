use winlex_core::{Architecture, Catalog, SearchMode, SearchOptions};

#[test]
fn declarations_cover_native_signatures_and_architecture_specific_types() {
    let catalog = Catalog::open(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../inputs/Windows.Win32.winmd"),
    )
    .unwrap();
    for arch in [Architecture::X86, Architecture::X64, Architecture::Arm64] {
        for name in ["CreateFileW", "RECT"] {
            let entity = catalog
                .search(
                    name,
                    SearchOptions {
                        mode: SearchMode::Exact,
                        architecture: Some(arch),
                        ..Default::default()
                    },
                )
                .unwrap()
                .next()
                .unwrap()
                .unwrap();
            let rendered = winlex_format::declaration::render(&catalog, entity.id, arch).unwrap();
            let utf16: Vec<_> = rendered.text.encode_utf16().collect();
            let mut end = 0;
            for number in &rendered.numbers {
                assert!(number.range.start >= end);
                end = number.range.end;
                let mut expected = format!("{}(0x{:X})", number.value, number.value);
                if let Some(unit) = number.unit {
                    expected.push(' ');
                    expected.push_str(unit);
                }
                assert_eq!(
                    String::from_utf16(&utf16[number.range.clone()]).unwrap(),
                    expected
                );
                assert!(catalog.entity(number.entity).is_ok());
            }
            let text = rendered.text;
            if name == "CreateFileW" {
                assert!(text.contains("CreateFileW("));
                assert!(text.contains("lpFileName"));
                assert!(text.contains("dwDesiredAccess"));
                assert!(text.contains("[in, const] PWSTR lpFileName"));
                assert!(text.contains("optional"));
                assert!(!text.contains("\"parameters\""));
            } else {
                assert!(text.contains("struct RECT"));
                assert!(text.contains("INT32 left;"));
                assert!(text.contains("INT32 bottom;"));
                assert!(text.contains("size: 16(0x10) bytes, alignment: 4(0x4) bytes"));
                assert!(text.contains("offset: 12(0xC) bytes, size: 4(0x4) bytes"));
            }
        }
        let render = |name| {
            let entity = catalog
                .search(
                    name,
                    SearchOptions {
                        mode: SearchMode::Exact,
                        architecture: Some(arch),
                        ..Default::default()
                    },
                )
                .unwrap()
                .next()
                .unwrap()
                .unwrap();
            winlex_format::declaration::render(&catalog, entity.id, arch)
                .unwrap()
                .text
        };
        let text = render("SECURITY_ATTRIBUTES");
        assert!(text.contains("UINT32 nLength;"), "{text}");
        assert!(text.contains("BOOL bInheritHandle;"), "{text}");
        let pointer_offset = if arch == Architecture::X86 {
            "4(0x4)"
        } else {
            "8(0x8)"
        };
        assert!(text.contains(&format!("offset: {pointer_offset} bytes, size: {pointer_offset} bytes\n    void *lpSecurityDescriptor;")), "{text}");
        let text = render("IUnknown");
        let release_offset = if arch == Architecture::X86 {
            "8(0x8)"
        } else {
            "16(0x10)"
        };
        assert!(
            text.contains(&format!(
                "slot 2(0x2), vtable +{release_offset} bytes\n    UINT32 Release("
            )),
            "{text}"
        );
        let text = render("IStream");
        assert!(
            text.contains("IUnknown::Release (inherited/implicit)"),
            "{text}"
        );
    }
    let entity = catalog
        .type_ids()
        .map(|id| catalog.entity(id).unwrap())
        .find(|e| e.architecture_mask == 4)
        .expect("ARM64-only fixture");
    assert!(winlex_format::declaration::render(&catalog, entity.id, Architecture::Arm64).is_ok());
    assert!(winlex_format::declaration::render(&catalog, entity.id, Architecture::X64).is_err());
}

#[test]
fn generic_inheritance_and_delegate_signatures_are_preserved() {
    use winlex_core::windows_metadata::{writer, *};
    let mut file = writer::File::new("DeclarationFixtures");
    let object = file.TypeRef("System", "Object");
    let base = file.TypeDef(
        "Fixture",
        "Base",
        writer::TypeDefOrRef::TypeRef(object),
        TypeAttributes::Public,
    );
    let derived = file.TypeDef(
        "Fixture",
        "Derived`1",
        writer::TypeDefOrRef::TypeDef(base),
        TypeAttributes::Public,
    );
    file.GenericParam(
        "T",
        writer::TypeOrMethodDef::TypeDef(derived),
        0,
        GenericParamAttributes(0),
    );
    file.MethodDef(
        "Convert",
        &Signature {
            flags: MethodCallAttributes::HASTHIS,
            return_type: Type::Generic("T".into(), 0),
            types: vec![Type::Generic("T".into(), 0)],
        },
        MethodAttributes::Public,
        MethodImplAttributes(0),
    );
    file.Param(
        "value",
        1,
        ParamAttributes::In | ParamAttributes::Out | ParamAttributes::Optional,
    );
    let delegate = file.TypeRef("System", "MulticastDelegate");
    file.TypeDef(
        "Fixture",
        "Callback",
        writer::TypeDefOrRef::TypeRef(delegate),
        TypeAttributes::Public | TypeAttributes::Sealed,
    );
    file.MethodDef(
        "Invoke",
        &Signature {
            return_type: Type::I32,
            types: vec![Type::U32],
            ..Default::default()
        },
        MethodAttributes::Public,
        MethodImplAttributes(0),
    );
    file.Param("value", 1, ParamAttributes::In);
    let interface = file.TypeDef(
        "Fixture",
        "IGeneric`1",
        writer::TypeDefOrRef::default(),
        TypeAttributes::Public | TypeAttributes::Interface | TypeAttributes::Abstract,
    );
    file.GenericParam(
        "T",
        writer::TypeOrMethodDef::TypeDef(interface),
        0,
        GenericParamAttributes(0),
    );
    let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tmp");
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join(format!("declaration-fixtures-{}.winmd", std::process::id()));
    std::fs::write(&path, file.into_stream()).unwrap();
    let catalog = Catalog::open(&path).unwrap();
    std::fs::remove_file(path).unwrap();
    let render = |name| {
        let entity = catalog
            .search(
                name,
                SearchOptions {
                    mode: SearchMode::Exact,
                    ..Default::default()
                },
            )
            .unwrap()
            .next()
            .unwrap()
            .unwrap();
        winlex_format::declaration::render(&catalog, entity.id, Architecture::X64)
            .unwrap()
            .text
    };
    let text = render("Derived`1");
    assert!(text.contains("class Derived<T> : Base"), "{text}");
    assert!(text.contains("T Convert("), "{text}");
    assert!(text.contains("[in, out, optional] T value"), "{text}");
    let text = render("Callback");
    assert!(text.starts_with("delegate INT32 Callback("), "{text}");
    assert!(!text.contains("Invoke"));
    let text = render("IGeneric`1");
    assert!(text.contains("Vtable unavailable:"), "{text}");
    assert!(text.contains("interface IGeneric<T>"), "{text}");
}
