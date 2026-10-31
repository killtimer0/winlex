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
                        mode: SearchMode::Name,
                        categories: Some(
                            winlex_core::EntityCategory::ALL
                                & !(winlex_core::EntityCategory::TypeReference as u32),
                        ),
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
                let region = String::from_utf16(&utf16[number.range.clone()]).unwrap();
                assert!(
                    region.contains(&number.value.to_string())
                        || region.contains(&number.hexadecimal()),
                    "{region}"
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
            } else {
                assert!(text.contains("struct RECT"), "{text}");
                assert!(text.contains("INT32 left;"));
                assert!(text.contains("INT32 bottom;"));
                assert!(text.contains("size: 16 bytes, alignment: 4 bytes"));
                assert!(text.contains("offset: 12 bytes, size: 4 bytes"));
            }
        }
        let render = |name| {
            let entity = catalog
                .search(
                    name,
                    SearchOptions {
                        mode: SearchMode::Name,
                        categories: Some(
                            winlex_core::EntityCategory::ALL
                                & !(winlex_core::EntityCategory::TypeReference as u32),
                        ),
                        architecture: Some(arch),
                        ..Default::default()
                    },
                )
                .unwrap()
                .next()
                .unwrap()
                .unwrap();
            winlex_format::declaration::render(&catalog, entity.id, arch).unwrap()
        };
        let rendered = render("SECURITY_ATTRIBUTES");
        let text = &rendered.text;
        assert!(text.contains("UINT32 nLength;"), "{text}");
        assert!(text.contains("BOOL bInheritHandle;"), "{text}");
        let pointer_offset = if arch == Architecture::X86 { "4" } else { "8" };
        assert!(text.contains(&format!("offset: {pointer_offset} bytes, size: {pointer_offset} bytes\n    void *lpSecurityDescriptor;")), "{text}");
        let rendered = render("IUnknown");
        let text = &rendered.text;
        let release_offset = if arch == Architecture::X86 {
            "0x8"
        } else {
            "0x10"
        };
        assert!(
            text.contains(&format!(
                "slot 2, vtable +{release_offset}\n    UINT32 Release("
            )),
            "{text}"
        );
        let units: Vec<_> = text.encode_utf16().collect();
        for (region, value, unit) in [
            ("slot 2".to_owned(), 2, None),
            (
                format!("vtable +{release_offset}"),
                if arch == Architecture::X86 { 8 } else { 16 },
                Some("bytes"),
            ),
        ] {
            let number = rendered
                .numbers
                .iter()
                .find(|n| String::from_utf16(&units[n.range.clone()]).unwrap() == region)
                .unwrap();
            assert_eq!(number.value, value);
            assert_eq!(number.unit, unit);
        }
        let rendered = render("IStream");
        let text = &rendered.text;
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
                    mode: SearchMode::Name,
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

#[test]
fn enum_members_and_individual_constants_share_annotations() {
    use winlex_core::windows_metadata::{writer, *};
    let mut file = writer::File::new("ConstantDeclarations");
    let base = file.TypeRef("System", "Enum");
    file.TypeDef(
        "Fixture",
        "Status",
        writer::TypeDefOrRef::TypeRef(base),
        TypeAttributes::Public,
    );
    file.Field("value__", &Type::I32, FieldAttributes::Public);
    let field = file.Field(
        "Failure",
        &Type::I32,
        FieldAttributes::Public | FieldAttributes::Static | FieldAttributes::Literal,
    );
    file.Constant(writer::HasConstant::Field(field), &Value::I32(-1073741819));
    let object = file.TypeRef("System", "Object");
    file.TypeDef(
        "Fixture",
        "Constants",
        writer::TypeDefOrRef::TypeRef(object),
        TypeAttributes::Public,
    );
    let field = file.Field(
        "Text",
        &Type::String,
        FieldAttributes::Public | FieldAttributes::Static | FieldAttributes::Literal,
    );
    file.Constant(
        writer::HasConstant::Field(field),
        &Value::Utf16("中😀\0\"\\\n".into()),
    );
    let field = file.Field(
        "EmptyText",
        &Type::String,
        FieldAttributes::Public | FieldAttributes::Static | FieldAttributes::Literal,
    );
    file.Constant(
        writer::HasConstant::Field(field),
        &Value::Utf16(String::new()),
    );
    let field = file.Field(
        "NullText",
        &Type::String,
        FieldAttributes::Public | FieldAttributes::Static | FieldAttributes::Literal,
    );
    file.Constant(writer::HasConstant::Field(field), &Value::U32(0));
    let mut bytes = file.into_stream();
    {
        use winlex_core::windows_metadata::reader::*;
        let index = Index::new(vec![File::new(bytes.clone()).unwrap()]);
        let constant = index
            .rows::<Field>(0)
            .find(|f| f.name() == "NullText")
            .unwrap()
            .constant()
            .unwrap();
        // Small ECMA-335 Constant rows have three two-byte columns.
        let row: Vec<_> = (0..3)
            .flat_map(|column| (constant.usize(column) as u16).to_le_bytes())
            .collect();
        let positions: Vec<_> = bytes
            .windows(row.len())
            .enumerate()
            .filter_map(|(offset, value)| (value == row).then_some(offset))
            .collect();
        assert_eq!(positions.len(), 1);
        bytes[positions[0]] = 0x12;
    }
    let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tmp");
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join(format!(
        "constant-declarations-{}.winmd",
        std::process::id()
    ));
    std::fs::write(&path, bytes).unwrap();
    let catalog = Catalog::open(&path).unwrap();
    std::fs::remove_file(path).unwrap();
    let render = |name| {
        let entity = catalog
            .search(
                name,
                SearchOptions {
                    mode: SearchMode::Name,
                    ..Default::default()
                },
            )
            .unwrap()
            .next()
            .unwrap()
            .unwrap();
        winlex_format::declaration::render(&catalog, entity.id, Architecture::X64).unwrap()
    };
    let parent = render("Status");
    let member = render("Failure");
    assert!(
        parent.text.contains("Failure = -1073741819, // 0xC0000005"),
        "{}",
        parent.text
    );
    assert!(member.text.contains("Failure = -1073741819; // 0xC0000005"));
    assert_eq!(member.numbers.len(), 2);
    for number in &member.numbers {
        assert_eq!(number.value, -1073741819);
        assert_eq!(number.hexadecimal(), "0xC0000005");
    }
    assert_eq!(parent.numbers.len(), 2);
    for (a, b) in parent.numbers.iter().zip(&member.numbers) {
        assert_eq!(a.entity, b.entity);
        assert_eq!(a.value, b.value);
        assert_eq!(a.hexadecimal(), b.hexadecimal());
    }
    let parent = render("Constants");
    let member = render("Text");
    assert!(
        parent
            .text
            .contains("\"中😀\\u0000\\\"\\\\\\n\"; // length: 7 UTF-16 code units"),
        "{}",
        parent.text
    );
    assert!(member.text.contains("length: 7 UTF-16 code units"));
    assert_eq!(member.numbers.len(), 1);
    assert_eq!(member.numbers[0].value, 7);
    assert_eq!(member.numbers[0].with_unit(), "7 UTF-16 code units");
    assert_eq!(parent.numbers[0].entity, member.numbers[0].entity);
    assert!(
        render("EmptyText")
            .text
            .contains("= \"\"; // length: 0 UTF-16 code units")
    );
    let null = render("NullText");
    assert!(null.text.contains("= null;"), "{}", null.text);
    assert!(null.numbers.is_empty());
}
