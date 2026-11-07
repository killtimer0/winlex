use crate::{
    Architecture,
    test_support::{catalog, type_id as id},
};
use windows_metadata::{writer, *};

#[test]
fn architecture_filter_requires_the_qualified_attribute_and_enum() {
    use crate::test_support::FixtureFile;
    let ns = "Windows.Win32.Foundation.Metadata";
    for (attribute_ns, value, mask) in [
        (
            ns,
            Value::EnumValue(TypeName::named(ns, "Architecture"), Box::new(Value::I32(2))),
            2,
        ),
        (
            "Other",
            Value::EnumValue(TypeName::named(ns, "Architecture"), Box::new(Value::I32(2))),
            7,
        ),
        ("Windows.Win32.Metadata", Value::I32(2), 7),
        (ns, Value::I32(2), 7),
        (
            ns,
            Value::EnumValue(
                TypeName::named("Other", "Architecture"),
                Box::new(Value::I32(2)),
            ),
            7,
        ),
    ] {
        let mut file = writer::File::new("ArchitectureContract");
        let attribute = file.TypeRef(attribute_ns, "SupportedArchitectureAttribute");
        let ctor = file.constructor(attribute, vec![value.ty()]);
        let ty = file.TypeDef(
            "Fixture",
            "Restricted",
            writer::TypeDefOrRef::default(),
            TypeAttributes::Public,
        );
        file.Attribute(
            writer::HasAttribute::TypeDef(ty),
            writer::AttributeType::MemberRef(ctor),
            &[(String::new(), value)],
        );
        let db = catalog("ArchitectureContract", file);
        assert_eq!(
            db.entity(id(&db, "Restricted")).unwrap().architecture_mask,
            mask
        );
        for arch in Architecture::ALL {
            let rows = db
                .search_exact(
                    "Restricted",
                    crate::SearchOptions {
                        architecture: Some(arch),
                        ..Default::default()
                    },
                )
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            assert_eq!(!rows.is_empty(), mask & arch.bit() != 0);
        }
    }
}

fn method(file: &mut writer::File, name: &str, flags: MethodAttributes) {
    file.MethodDef(
        name,
        &Signature {
            flags: MethodCallAttributes::HASTHIS,
            return_type: Type::Void,
            types: vec![],
        },
        MethodAttributes::Public | flags,
        MethodImplAttributes::default(),
    );
}

#[test]
fn native_and_winrt_tables_preserve_headers_slots_and_provenance() {
    let mut file = writer::File::new("VtableFixture");
    let flags = TypeAttributes::Public | TypeAttributes::Interface | TypeAttributes::Abstract;
    file.TypeDef("Fixture", "Base", writer::TypeDefOrRef::default(), flags);
    method(&mut file, "BaseMethod", MethodAttributes::Virtual);
    let derived = file.TypeDef("Fixture", "Derived", writer::TypeDefOrRef::default(), flags);
    file.InterfaceImpl(
        derived,
        &Type::ClassName(TypeName::named("Fixture", "Base")),
    );
    method(&mut file, "DerivedMethod", MethodAttributes::Virtual);
    let runtime = file.TypeDef(
        "Fixture",
        "Runtime",
        writer::TypeDefOrRef::default(),
        flags | TypeAttributes::WindowsRuntime,
    );
    file.InterfaceImpl(
        runtime,
        &Type::ClassName(TypeName::named("Fixture", "Base")),
    );
    method(&mut file, "RuntimeMethod", MethodAttributes::Virtual);
    let delegate_base = file.TypeRef("System", "MulticastDelegate");
    file.TypeDef(
        "Fixture",
        "Callback",
        writer::TypeDefOrRef::TypeRef(delegate_base),
        TypeAttributes::Public | TypeAttributes::Sealed | TypeAttributes::WindowsRuntime,
    );
    method(&mut file, ".ctor", MethodAttributes::SpecialName);
    method(&mut file, "Invoke", MethodAttributes::Virtual);
    let db = catalog("VtableFixture", file);
    for arch in [Architecture::X86, Architecture::X64, Architecture::Arm64] {
        for (name, winrt, expected) in [
            ("Derived", false, vec!["BaseMethod", "DerivedMethod"]),
            (
                "Runtime",
                true,
                vec![
                    "QueryInterface",
                    "AddRef",
                    "Release",
                    "GetIids",
                    "GetRuntimeClassName",
                    "GetTrustLevel",
                    "RuntimeMethod",
                ],
            ),
            (
                "Callback",
                true,
                vec!["QueryInterface", "AddRef", "Release", "Invoke"],
            ),
        ] {
            let entity = id(&db, name);
            let table = db.vtable(entity, arch).unwrap();
            assert_eq!(table.id, entity);
            assert_eq!(table.name, name);
            assert_eq!(table.architecture, arch);
            assert_eq!(table.windows_runtime, winrt);
            assert_eq!(
                table
                    .slots
                    .iter()
                    .map(|s| s.method.as_str())
                    .collect::<Vec<_>>(),
                expected
            );
            for slot in &table.slots {
                if let Some(method) = slot.id {
                    assert_eq!(db.entity(method).unwrap().name, slot.method);
                    assert_eq!(
                        db.entity(slot.declaring_type_id.unwrap()).unwrap().name,
                        slot.declaring_type
                    );
                } else {
                    assert!(slot.declaring_type_id.is_none());
                    assert_eq!(
                        slot.declaring_type,
                        if slot.method.starts_with("Get") {
                            "IInspectable"
                        } else {
                            "IUnknown"
                        }
                    );
                }
            }
        }
    }
}

#[test]
fn vtable_construction_keeps_native_and_runtime_rejections() {
    for runtime in [false, true] {
        for (method_name, static_method, expected) in [
            ("Invoke", true, "static interface method unsupported"),
            ("Other", false, "WinRT delegate must have one Invoke method"),
        ] {
            if !runtime && !static_method {
                continue;
            }
            let mut file = writer::File::new("InvalidVtableFixture");
            let base = file.TypeRef(
                "System",
                if runtime {
                    "MulticastDelegate"
                } else {
                    "Object"
                },
            );
            let flags = if runtime {
                TypeAttributes::Public | TypeAttributes::Sealed | TypeAttributes::WindowsRuntime
            } else {
                TypeAttributes::Public | TypeAttributes::Interface | TypeAttributes::Abstract
            };
            file.TypeDef(
                "Fixture",
                "Invalid",
                if runtime {
                    writer::TypeDefOrRef::TypeRef(base)
                } else {
                    writer::TypeDefOrRef::default()
                },
                flags,
            );
            method(
                &mut file,
                method_name,
                if static_method {
                    MethodAttributes::Static
                } else {
                    MethodAttributes::Virtual
                },
            );
            let db = catalog("VtableFixture", file);
            let error = db
                .vtable(id(&db, "Invalid"), Architecture::X64)
                .err()
                .unwrap();
            assert_eq!(error.to_string(), expected);
        }
    }
}
