use winlex_core::{Architecture, Catalog, SearchMode, SearchOptions};

include!("../../../tests/rust_search.rs");
include!("support/source.rs");

fn render(catalog: &Catalog, name: &str) -> winlex_format::declaration::Rendered {
    let entity = first_match(catalog.search(
        name,
        SearchOptions {
            mode: SearchMode::Name,
            ..Default::default()
        },
    ));
    winlex_format::declaration::render(catalog, entity.id, Architecture::X64).unwrap()
}

#[test]
fn signature_types_use_metadata_names_and_semantic_regions() {
    use winlex_core::windows_metadata::{writer, *};
    use winlex_format::declaration::{RegionKind, SymbolRole};
    let mut file = writer::File::new("SignatureTypes");
    let base = file.TypeRef("System", "ValueType");
    file.TypeDef(
        "Fixture",
        "Record😀",
        writer::TypeDefOrRef::TypeRef(base),
        TypeAttributes::Public | TypeAttributes::SequentialLayout,
    );
    let cases = [
        (Type::Bool, "bool"),
        (Type::Char, "char"),
        (Type::I8, "int8"),
        (Type::U8, "uint8"),
        (Type::I16, "int16"),
        (Type::U16, "uint16"),
        (Type::I32, "int32"),
        (Type::U32, "uint32"),
        (Type::I64, "int64"),
        (Type::U64, "uint64"),
        (Type::F32, "float32"),
        (Type::F64, "float64"),
        (Type::ISize, "nint"),
        (Type::USize, "nuint"),
        (Type::String, "string"),
        (Type::Object, "object"),
        (Type::value_named("System", "Guid"), "Guid"),
        (Type::value_named("Missing", "INT32"), "INT32"),
    ];
    for (i, (ty, _)) in cases.iter().enumerate() {
        file.Field(&format!("field{i}"), ty, FieldAttributes::Public);
    }
    let catalog = Catalog::from_inputs([source("signature-types", file.into_stream())]).unwrap();
    for (i, (_, expected)) in cases.iter().enumerate() {
        let rendered = render(&catalog, &format!("field{i}"));
        assert_eq!(rendered.text, format!("{expected} field{i};\n"));
        assert_eq!(rendered.regions.len(), 2);
        assert!(matches!(rendered.regions[0].kind, RegionKind::Symbol));
        assert_eq!(rendered.regions[0].role, Some(SymbolRole::Type));
        assert_eq!(
            rendered.regions[0].range,
            0..expected.encode_utf16().count()
        );
        assert!(matches!(rendered.regions[1].kind, RegionKind::Symbol));
        assert_eq!(rendered.regions[1].role, Some(SymbolRole::Member));
        assert_eq!(
            rendered.regions[1].range.start,
            expected.encode_utf16().count() + 1
        );
    }
}

#[test]
fn method_imports_and_variadic_signatures_preserve_metadata_meaning() {
    use winlex_core::windows_metadata::{writer, *};
    let mut file = writer::File::new("MethodPresentation");
    file.TypeDef(
        "Fixture",
        "Apis",
        writer::TypeDefOrRef::default(),
        TypeAttributes::Public,
    );
    for (name, flags, parameters, import) in [
        ("Print", 5, 1, Some(("", "printf", 0x200))),
        ("Imported", 0, 0, Some(("library.dll", "entry", 0x100))),
        ("UnknownConvention", 0, 0, Some(("", "entry", 0))),
        ("OnlyVariadic", 5, 0, None),
        ("InstanceVariadic", 0x25, 1, None),
        ("VaList", 0, 2, None),
        ("OtherConvention", 1, 1, None),
    ] {
        let attributes = if MethodCallAttributes(flags).contains(MethodCallAttributes::HASTHIS) {
            MethodAttributes::Public
        } else {
            MethodAttributes::Public | MethodAttributes::Static
        };
        let method = file.MethodDef(
            name,
            &Signature {
                flags: MethodCallAttributes(flags),
                return_type: Type::I32,
                types: vec![Type::I32; parameters],
            },
            attributes,
            MethodImplAttributes(0),
        );
        if let Some((module, entry, convention)) = import {
            file.ImplMap(method, PInvokeAttributes(convention), entry, module);
        }
    }
    let catalog =
        Catalog::from_inputs([source("method-presentation", file.into_stream())]).unwrap();
    for (name, expected) in [
        (
            "Print",
            "// printf; C\nint32 Print(\n    int32 arg1,\n    ...\n);\n",
        ),
        (
            "Imported",
            "// library.dll!entry; system\nint32 Imported(void);\n",
        ),
        (
            "UnknownConvention",
            "// entry\nint32 UnknownConvention(void);\n",
        ),
        ("OnlyVariadic", "static int32 Apis::OnlyVariadic(...);\n"),
        (
            "InstanceVariadic",
            "// this: rcx\nint32 Apis::InstanceVariadic(\n    int32 arg1, // rdx\n    ...\n); // return: rax\n",
        ),
        (
            "VaList",
            "static int32 Apis::VaList(\n    int32 arg1,\n    int32 arg2\n);\n",
        ),
        (
            "OtherConvention",
            "static int32 Apis::OtherConvention(\n    int32 arg1\n);\n",
        ),
    ] {
        assert_eq!(render(&catalog, name).text, expected, "{name}");
    }
}

#[test]
fn method_declarations_mark_static_and_declaring_type_only_at_the_root() {
    use winlex_core::windows_metadata::{writer, *};
    use winlex_format::declaration::{RegionKind, SymbolRole};
    let mut file = writer::File::new("MethodMarkers");
    file.TypeDef(
        "Fixture",
        "Owner",
        writer::TypeDefOrRef::default(),
        TypeAttributes::Public,
    );
    let signature = Signature {
        flags: MethodCallAttributes::default(),
        return_type: Type::Void,
        types: vec![],
    };
    file.MethodDef(
        "Run",
        &signature,
        MethodAttributes::Public,
        MethodImplAttributes(0),
    );
    file.MethodDef(
        "Build",
        &signature,
        MethodAttributes::Public | MethodAttributes::Static,
        MethodImplAttributes::default(),
    );
    let native = file.MethodDef(
        "Native",
        &signature,
        MethodAttributes::Public | MethodAttributes::Static,
        MethodImplAttributes::default(),
    );
    file.ImplMap(native, PInvokeAttributes(0), "Native", "library.dll");
    let catalog = Catalog::from_inputs([source("method-markers", file.into_stream())]).unwrap();

    let run = render(&catalog, "Run");
    assert_eq!(run.text, "// this: rcx\nvoid Owner::Run(void);\n");
    assert_eq!(run.regions.len(), 3);
    assert_eq!(run.regions[1].range, 18..23);
    assert_eq!(run.regions[1].role, Some(SymbolRole::Type));
    assert_eq!(
        catalog
            .entity(run.regions[1].reference().unwrap())
            .unwrap()
            .full_name,
        "Fixture.Owner"
    );
    assert_eq!(run.regions[2].range, 25..28);
    assert_eq!(run.regions[2].role, Some(SymbolRole::Method));
    assert!(run.regions[2].reference().is_none());
    let build = render(&catalog, "Build");
    assert_eq!(build.text, "static void Owner::Build(void);\n");
    assert_eq!(build.regions.len(), 3);
    assert_eq!(build.regions[1].range, 12..17);
    assert!(matches!(build.regions[1].kind, RegionKind::Reference(_)));
    assert_eq!(build.regions[2].range, 19..24);
    assert_eq!(build.regions[2].role, Some(SymbolRole::Method));
    let native = render(&catalog, "Native");
    assert_eq!(native.text, "// library.dll!Native\nvoid Native(void);\n");
    assert!(
        native
            .regions
            .iter()
            .all(|region| !matches!(region.kind, RegionKind::Reference(_)))
    );
    assert_eq!(
        native
            .regions
            .iter()
            .find(|region| region.role == Some(SymbolRole::Method))
            .unwrap()
            .range,
        27..33
    );

    let owner = render(&catalog, "Owner");
    assert!(
        owner.text.contains("    void Run(void);\n"),
        "{}",
        owner.text
    );
    assert!(
        owner.text.contains("    static void Build(void);\n"),
        "{}",
        owner.text
    );
    assert!(
        owner.text.contains("    void Native(void);\n"),
        "{}",
        owner.text
    );
    for name in ["Run", "Build", "Native"] {
        let member = owner
            .regions
            .iter()
            .find(|region| {
                region.role == Some(SymbolRole::Method)
                    && region
                        .reference()
                        .is_some_and(|id| catalog.entity(id).unwrap().name == name)
            })
            .unwrap_or_else(|| panic!("missing member link {name}: {}", owner.text));
        assert!(matches!(member.kind, RegionKind::Reference(_)));
    }
}

#[test]
fn generic_method_owners_link_the_type_but_not_its_parameters() {
    use winlex_core::windows_metadata::{writer, *};
    use winlex_format::declaration::{RegionKind, SymbolRole};
    let mut file = writer::File::new("GenericMethodOwner");
    let owner = file.TypeDef(
        "Fixture",
        "Box`1",
        writer::TypeDefOrRef::default(),
        TypeAttributes::Public,
    );
    file.GenericParam(
        "T",
        writer::TypeOrMethodDef::TypeDef(owner),
        0,
        GenericParamAttributes(0),
    );
    file.MethodDef(
        "Run",
        &Signature {
            flags: MethodCallAttributes::default(),
            return_type: Type::Generic("T".into(), 0),
            types: vec![],
        },
        MethodAttributes::Public | MethodAttributes::Static,
        MethodImplAttributes(0),
    );
    let catalog =
        Catalog::from_inputs([source("generic-method-owner", file.into_stream())]).unwrap();

    let rendered = render(&catalog, "Run");
    assert_eq!(rendered.text, "static T Box<T>::Run(void);\n");
    assert_eq!(rendered.regions.len(), 4);
    assert_eq!(rendered.regions[0].range, 7..8);
    assert!(matches!(rendered.regions[0].kind, RegionKind::Symbol));
    assert_eq!(rendered.regions[0].role, Some(SymbolRole::Type));
    assert_eq!(rendered.regions[1].range, 9..12);
    assert_eq!(rendered.regions[1].role, Some(SymbolRole::Type));
    assert_eq!(
        catalog
            .entity(rendered.regions[1].reference().unwrap())
            .unwrap()
            .full_name,
        "Fixture.Box`1"
    );
    assert_eq!(rendered.regions[2].range, 13..14);
    assert!(matches!(rendered.regions[2].kind, RegionKind::Symbol));
    assert_eq!(rendered.regions[2].role, Some(SymbolRole::Type));
    assert_eq!(rendered.regions[3].range, 17..20);
    assert_eq!(rendered.regions[3].role, Some(SymbolRole::Method));
    assert!(rendered.regions[3].reference().is_none());
}

#[test]
fn external_references_remain_unlinked_when_missing_or_ambiguous() {
    use winlex_core::windows_metadata::{reader, writer, *};
    use winlex_format::declaration::SymbolRole;
    let mut dependency = writer::File::new("NavigationExternal");
    dependency.TypeDef(
        "External",
        "Target",
        writer::TypeDefOrRef::default(),
        TypeAttributes::Public | TypeAttributes::WindowsRuntime,
    );
    let bytes = dependency.into_stream();
    let mut consumer = writer::File::new("NavigationConsumer");
    consumer.set_reference(reader::Index::new(vec![
        reader::File::new(bytes.clone()).unwrap(),
    ]));
    consumer.TypeDef(
        "Fixture",
        "Owner",
        writer::TypeDefOrRef::default(),
        TypeAttributes::Public,
    );
    consumer.Field(
        "Member",
        &Type::class_named("External", "Target"),
        FieldAttributes::Public,
    );
    let inputs = [
        source("navigation-external-consumer", consumer.into_stream()),
        source("navigation-external-dependency", bytes),
    ];
    let missing = Catalog::from_inputs([inputs[0].clone()]).unwrap();
    let resolved = Catalog::from_inputs(inputs.clone()).unwrap();
    let ambiguous =
        Catalog::from_inputs([inputs[0].clone(), inputs[1].clone(), inputs[1].clone()]).unwrap();
    for (catalog, expected) in [
        (&missing, false),
        (&resolved, true),
        (&ambiguous, false),
        (&missing, false),
    ] {
        let rendered = render(catalog, "Member");
        assert_eq!(rendered.text, "Target Member;\n");
        assert_eq!(rendered.regions.len(), 2);
        assert_eq!(rendered.regions[0].range, 0..6);
        assert_eq!(rendered.regions[0].role, Some(SymbolRole::Type));
        assert_eq!(rendered.regions[0].reference().is_some(), expected);
        assert_eq!(rendered.regions[1].range, 7..13);
        assert_eq!(rendered.regions[1].role, Some(SymbolRole::Member));
        assert!(rendered.regions[1].reference().is_none());
        if expected {
            let target = rendered.regions[0].reference().unwrap();
            assert_eq!(target.source_id, 1);
            assert_eq!(catalog.entity(target).unwrap().full_name, "External.Target");
            assert_eq!(rendered.regions[0].range, 0..6);
        }
    }
}

#[test]
fn named_references_keep_identity_through_declarators_generics_and_utf16() {
    use winlex_core::windows_metadata::{writer, *};
    use winlex_format::declaration::SymbolRole;
    let mut file = writer::File::new("NavigationDeclarations");
    for namespace in ["Left", "Right"] {
        file.TypeDef(
            namespace,
            "Item",
            writer::TypeDefOrRef::default(),
            TypeAttributes::Public,
        );
    }
    let generic = file.TypeDef(
        "Fixture",
        "Box`1",
        writer::TypeDefOrRef::default(),
        TypeAttributes::Public,
    );
    file.GenericParam(
        "T",
        writer::TypeOrMethodDef::TypeDef(generic),
        0,
        GenericParamAttributes(0),
    );
    let base = file.TypeDef(
        "Fixture",
        "Base",
        writer::TypeDefOrRef::default(),
        TypeAttributes::Public,
    );
    let owner = file.TypeDef(
        "Fixture",
        "Owner😀",
        writer::TypeDefOrRef::TypeDef(base),
        TypeAttributes::Public,
    );
    file.InterfaceImpl(owner, &Type::class_named("Left", "Item"));
    let mut generic_name = TypeName::named("Fixture", "Box`1");
    generic_name.generics = vec![Type::value_named("Right", "Item")];
    let nested = Type::PtrConst(
        Box::new(Type::ArrayFixed(
            Box::new(Type::ValueName(generic_name.clone())),
            3,
        )),
        1,
    );
    file.Field("值😀", &nested, FieldAttributes::Public);
    let property = file.Property("Current", &Type::ValueName(generic_name));
    file.PropertyMap(owner, property);
    let event = file.Event("Changed", &Type::class_named("Right", "Item"));
    file.EventMap(owner, event);
    file.MethodDef(
        "Call",
        &Signature {
            return_type: Type::PtrMut(Box::new(Type::value_named("Left", "Item")), 1),
            types: vec![nested, Type::class_named("Missing", "Unknown")],
            ..Default::default()
        },
        MethodAttributes::Public,
        MethodImplAttributes(0),
    );
    file.Param("参数😀", 1, ParamAttributes::In);
    file.Param("missing", 2, ParamAttributes::default());
    let catalog =
        Catalog::from_inputs([source("navigation-declarations", file.into_stream())]).unwrap();
    let owner = catalog
        .type_ids()
        .find(|&id| catalog.entity(id).unwrap().name == "Owner😀")
        .unwrap();
    let rendered = winlex_format::declaration::render(&catalog, owner, Architecture::X64).unwrap();
    assert!(
        rendered.text.contains("class Owner😀 : Base, Item\n{"),
        "{}",
        rendered.text
    );
    assert!(
        rendered.text.contains("const Box<Item> (*值😀)[3];"),
        "{}",
        rendered.text
    );
    assert!(
        rendered.text.contains(concat!(
            "Item *Call(\n",
            "        [in] const Box<Item> (*参数😀)[3],\n",
            "        Unknown missing\n",
            "    );",
        )),
        "{}",
        rendered.text,
    );
    let units: Vec<_> = rendered.text.encode_utf16().collect();
    let links: Vec<_> = rendered
        .regions
        .iter()
        .filter_map(|r| {
            r.reference().map(|id| {
                (
                    String::from_utf16(&units[r.range.clone()]).unwrap(),
                    catalog.entity(id).unwrap().full_name,
                )
            })
        })
        .collect();
    assert_eq!(
        links,
        [
            ("Base", "Fixture.Base"),
            ("Item", "Left.Item"),
            ("Box", "Fixture.Box`1"),
            ("Item", "Right.Item"),
            ("值😀", "Fixture.Owner😀.值😀"),
            ("Box", "Fixture.Box`1"),
            ("Item", "Right.Item"),
            ("Current", "Fixture.Owner😀.Current"),
            ("Item", "Right.Item"),
            ("Changed", "Fixture.Owner😀.Changed"),
            ("Item", "Left.Item"),
            ("Call", "Fixture.Owner😀.Call"),
            ("Box", "Fixture.Box`1"),
            ("Item", "Right.Item"),
        ]
        .map(|(text, name)| (text.to_owned(), name.to_owned()))
    );
    for pair in rendered.regions.windows(2) {
        assert!(pair[0].range.end <= pair[1].range.start);
    }
    let role_of = |name: &str| {
        rendered
            .regions
            .iter()
            .find(|region| {
                region.reference().is_some()
                    && String::from_utf16(&units[region.range.clone()]).unwrap() == name
            })
            .unwrap_or_else(|| panic!("missing link {name}"))
            .role
            .expect("link role")
    };
    assert_eq!(role_of("Current"), SymbolRole::Member);
    assert_eq!(role_of("Changed"), SymbolRole::Member);
    assert_eq!(role_of("值😀"), SymbolRole::Member);
    assert_eq!(role_of("Call"), SymbolRole::Method);
    let reference = first_match(catalog.search(
        "Left.Item",
        SearchOptions {
            mode: SearchMode::Name,
            categories: Some(winlex_core::EntityCategory::TypeReference as u32),
            ..Default::default()
        },
    ));
    let resolved =
        winlex_format::declaration::render(&catalog, reference.id, Architecture::X64).unwrap();
    let link = resolved
        .regions
        .iter()
        .find(|r| r.reference().is_some())
        .unwrap();
    let units: Vec<_> = resolved.text.encode_utf16().collect();
    assert_eq!(
        String::from_utf16(&units[link.range.clone()]).unwrap(),
        "Left.Item"
    );
    assert_eq!(
        catalog.entity(link.reference().unwrap()).unwrap().full_name,
        "Left.Item"
    );
}

#[test]
fn parameter_modifiers_preserve_order_and_missing_parameter_names() {
    use winlex_core::windows_metadata::{writer, *};
    let mut file = writer::File::new("ParameterModifiers");
    let object = file.TypeRef("System", "Object");
    let constructors: Vec<_> = ["ReservedAttribute", "RetValAttribute", "ConstAttribute"]
        .into_iter()
        .map(|name| {
            let ty = file.TypeRef("Windows.Win32.Foundation.Metadata", name);
            file.MemberRef(
                ".ctor",
                &Signature {
                    flags: MethodCallAttributes::HASTHIS,
                    return_type: Type::Void,
                    types: vec![],
                },
                writer::MemberRefParent::TypeRef(ty),
            )
        })
        .collect();
    file.TypeDef(
        "Fixture",
        "Api",
        writer::TypeDefOrRef::TypeRef(object),
        TypeAttributes::Public,
    );
    let cases = [
        (0, "int32 值😀,"),
        (3, "[in, out] int32 值😀,"),
        (
            63,
            "[in, out, optional, reserved, retval, const] int32 值😀,",
        ),
    ];
    for (mask, _) in cases {
        file.MethodDef(
            &format!("Call{mask}"),
            &Signature {
                flags: MethodCallAttributes::default(),
                return_type: Type::Void,
                types: vec![Type::I32, Type::I32],
            },
            MethodAttributes::Public | MethodAttributes::Static,
            MethodImplAttributes::default(),
        );
        let mut flags = ParamAttributes::default();
        for (bit, flag) in [
            (1, ParamAttributes::In),
            (2, ParamAttributes::Out),
            (4, ParamAttributes::Optional),
        ] {
            if mask & bit != 0 {
                flags |= flag;
            }
        }
        let param = file.Param("值😀", 1, flags);
        for (index, &constructor) in constructors.iter().enumerate() {
            if mask & (8 << index) != 0 {
                file.Attribute(
                    writer::HasAttribute::Param(param),
                    writer::AttributeType::MemberRef(constructor),
                    &[],
                );
            }
        }
    }
    let catalog =
        Catalog::from_inputs([source("parameter-modifiers", file.into_stream())]).unwrap();
    for (mask, expected) in cases {
        let rendered = render(&catalog, &format!("Call{mask}"));
        assert!(
            rendered.text.lines().any(|line| line.trim() == expected),
            "{}",
            rendered.text
        );
        assert!(rendered.text.contains("int32 arg2"), "{}", rendered.text);
    }
}

#[test]
fn declarations_cover_native_signatures_and_architecture_specific_types() {
    let catalog = Catalog::open(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../inputs/Windows.Win32.winmd"),
    )
    .unwrap();
    for arch in [Architecture::X86, Architecture::X64, Architecture::Arm64] {
        let render = |name| {
            let entity = first_match(catalog.search(
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
            ));
            winlex_format::declaration::render(&catalog, entity.id, arch).unwrap()
        };
        for name in ["CreateFileW", "RECT"] {
            let rendered = render(name);
            let utf16: Vec<_> = rendered.text.encode_utf16().collect();
            let mut end = 0;
            for annotation in &rendered.regions {
                assert!(annotation.range.start >= end);
                end = annotation.range.end;
                let Some(number) = annotation.number() else {
                    continue;
                };
                let region = String::from_utf16(&utf16[annotation.range.clone()]).unwrap();
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
                assert!(text.contains("int32 left;"));
                assert!(text.contains("int32 bottom;"));
                assert!(text.contains("size: 16 bytes, alignment: 4 bytes"));
                assert!(text.contains("offset: 12 bytes, size: 4 bytes"));
            }
        }
        let rendered = render("SECURITY_ATTRIBUTES");
        let text = &rendered.text;
        assert!(text.contains("uint32 nLength;"), "{text}");
        assert!(text.contains("BOOL bInheritHandle;"), "{text}");
        let pointer_offset = if arch == Architecture::X86 { "4" } else { "8" };
        assert!(
            text.contains(&format!(
                "offset: {pointer_offset} bytes, size: {pointer_offset} bytes\n    void *lpSecurityDescriptor;"
            )),
            "{text}",
        );
        let rendered = render("IUnknown");
        let text = &rendered.text;
        let release_offset = if arch == Architecture::X86 {
            "0x8"
        } else {
            "0x10"
        };
        assert!(
            text.contains(&format!(
                "slot 2, vtable +{release_offset}\n    uint32 Release("
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
                .regions
                .iter()
                .find(|n| String::from_utf16(&units[n.range.clone()]).unwrap() == region)
                .unwrap()
                .number()
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
    let catalog =
        Catalog::from_inputs([source("declaration-fixtures", file.into_stream())]).unwrap();
    let text = render(&catalog, "Derived`1").text;
    assert!(text.contains("class Derived<T> : Base"), "{text}");
    assert!(text.contains("T Convert("), "{text}");
    assert!(text.contains("[in, out, optional] T value"), "{text}");
    let text = render(&catalog, "Callback").text;
    assert!(text.starts_with("delegate int32 Callback("), "{text}");
    assert!(!text.contains("Invoke"));
    let text = render(&catalog, "IGeneric`1").text;
    assert!(text.contains("Vtable unavailable:"), "{text}");
    assert!(text.contains("interface IGeneric<T>"), "{text}");
}

#[test]
fn enum_members_and_individual_constants_share_annotations() {
    use winlex_core::windows_metadata::{writer, *};
    use winlex_format::declaration::SymbolRole;
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
    let catalog = Catalog::from_inputs([source("constant-declarations", bytes)]).unwrap();
    let render = |name| render(&catalog, name);
    let parent = render("Status");
    let member = render("Failure");
    assert!(
        parent.text.contains("Failure = -1073741819, // 0xC0000005"),
        "{}",
        parent.text
    );
    assert!(member.text.contains("Failure = -1073741819; // 0xC0000005"));
    fn constant(
        rendered: &winlex_format::declaration::Rendered,
    ) -> &winlex_format::declaration::Region {
        rendered
            .regions
            .iter()
            .find(|region| region.role == Some(SymbolRole::Member))
            .expect("enum constant member region")
    }
    let units: Vec<_> = parent.text.encode_utf16().collect();
    let parent_constant = constant(&parent);
    assert_eq!(
        String::from_utf16(&units[parent_constant.range.clone()]).unwrap(),
        "Failure"
    );
    assert_eq!(
        catalog
            .entity(parent_constant.reference().unwrap())
            .unwrap()
            .name,
        "Failure"
    );
    assert!(constant(&member).reference().is_none());
    assert_eq!(member.regions.iter().filter_map(|r| r.number()).count(), 2);
    for number in member.regions.iter().filter_map(|r| r.number()) {
        assert_eq!(number.value, -1073741819);
        assert_eq!(number.hexadecimal(), "0xC0000005");
    }
    assert_eq!(parent.regions.iter().filter_map(|r| r.number()).count(), 2);
    for (a, b) in parent
        .regions
        .iter()
        .filter_map(|r| r.number())
        .zip(member.regions.iter().filter_map(|r| r.number()))
    {
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
    let numbers: Vec<_> = member.regions.iter().filter_map(|r| r.number()).collect();
    assert_eq!(numbers.len(), 1);
    assert_eq!(numbers[0].value, 7);
    assert_eq!(numbers[0].with_unit(), "7 UTF-16 code units");
    assert_eq!(
        parent
            .regions
            .iter()
            .find_map(|r| r.number())
            .unwrap()
            .entity,
        numbers[0].entity
    );
    assert!(
        render("EmptyText")
            .text
            .contains("= \"\"; // length: 0 UTF-16 code units")
    );
    let null = render("NullText");
    assert!(null.text.contains("= null;"), "{}", null.text);
    assert!(null.regions.iter().all(|r| r.number().is_none()));
}

#[test]
fn fastcall_functions_annotate_x86_registers_and_stack_slots() {
    use winlex_core::windows_metadata::{writer, *};
    let mut file = writer::File::new("NativeAbiFast");
    let base = file.TypeRef("System", "Object");
    file.TypeDef(
        "Fixture",
        "Apis",
        writer::TypeDefOrRef::TypeRef(base),
        TypeAttributes::Public,
    );
    let method = file.MethodDef(
        "Mix",
        &Signature {
            flags: MethodCallAttributes::default(),
            return_type: Type::Void,
            types: vec![Type::I32, Type::F32, Type::F64, Type::I32],
        },
        MethodAttributes::Public | MethodAttributes::Static,
        MethodImplAttributes::Native | MethodImplAttributes::Unmanaged,
    );
    for (index, name) in ["a", "b", "c", "d"].iter().enumerate() {
        file.Param(name, index as u16 + 1, ParamAttributes::In);
    }
    let attribute = file.TypeRef("Winlex.Metadata", "NativeCallingConventionAttribute");
    let constructor = file.MemberRef(
        ".ctor",
        &Signature {
            flags: MethodCallAttributes::HASTHIS,
            return_type: Type::Void,
            types: vec![Type::String],
        },
        writer::MemberRefParent::TypeRef(attribute),
    );
    file.Attribute(
        writer::HasAttribute::MethodDef(method),
        writer::AttributeType::MemberRef(constructor),
        &[(String::new(), Value::Utf8("fastcall".into()))],
    );

    let catalog = Catalog::from_inputs([source("native-abi-fast", file.into_stream())]).unwrap();
    let entity = first_match(catalog.search(
        "Mix",
        SearchOptions {
            mode: SearchMode::Name,
            ..Default::default()
        },
    ));
    assert_eq!(
        winlex_format::declaration::render(&catalog, entity.id, Architecture::X86)
            .unwrap()
            .text,
        "void Mix(\n    [in] int32 a, // ecx\n    [in] float32 b, // [esp+4]\n    [in] float64 c, // [esp+8]\n    [in] int32 d // edx\n);\n"
    );
}

#[test]
fn native_callables_annotate_parameters_and_hidden_returns() {
    use winlex_core::windows_metadata::{writer, *};
    let mut file = writer::File::new("NativeAbi");
    let value_type = file.TypeRef("System", "ValueType");
    file.TypeDef(
        "Fixture",
        "Record",
        writer::TypeDefOrRef::TypeRef(value_type),
        TypeAttributes::Public | TypeAttributes::SequentialLayout,
    );
    for index in 0..4 {
        file.Field(&format!("f{index}"), &Type::I32, FieldAttributes::Public);
    }

    file.TypeDef(
        "Fixture",
        "IThing",
        writer::TypeDefOrRef::default(),
        TypeAttributes::Public | TypeAttributes::Interface | TypeAttributes::Abstract,
    );
    let parameters = 5;
    file.MethodDef(
        "Annotate",
        &Signature {
            flags: MethodCallAttributes::HASTHIS,
            return_type: Type::value_named("Fixture", "Record"),
            types: vec![Type::I32; parameters],
        },
        MethodAttributes::Public | MethodAttributes::Abstract | MethodAttributes::Virtual,
        MethodImplAttributes(0),
    );
    for index in 1..=parameters {
        file.Param(&format!("p{index}"), index as u16, ParamAttributes::In);
    }

    let mut unannotated = writer::File::new("NativeAbiSkip");
    let base = unannotated.TypeRef("System", "Object");
    unannotated.TypeDef(
        "Fixture",
        "Apis",
        writer::TypeDefOrRef::TypeRef(base),
        TypeAttributes::Public,
    );
    let method = unannotated.MethodDef(
        "Copy",
        &Signature {
            flags: MethodCallAttributes::default(),
            return_type: Type::Void,
            types: vec![Type::Array(Box::new(Type::I32))],
        },
        MethodAttributes::Public | MethodAttributes::Static,
        MethodImplAttributes(0),
    );
    unannotated.ImplMap(
        method,
        PInvokeAttributes::CallConvPlatformapi,
        "Copy",
        "fixture.dll",
    );

    let catalog = Catalog::from_inputs([
        source("native-abi", file.into_stream()),
        source("native-abi-skip", unannotated.into_stream()),
    ])
    .unwrap();
    assert_eq!(
        render(&catalog, "Annotate").text,
        "// this: rcx\n// return buffer: rdx\nRecord IThing::Annotate(\n    [in] int32 p1, // r8\n    [in] int32 p2, // r9\n    [in] int32 p3, // [rsp+0x28]\n    [in] int32 p4, // [rsp+0x30]\n    [in] int32 p5 // [rsp+0x38]\n);\n"
    );
    assert_eq!(
        render(&catalog, "Copy").text,
        "// fixture.dll!Copy; system\nvoid Copy(\n    int32 arg1[]\n);\n"
    );
}
