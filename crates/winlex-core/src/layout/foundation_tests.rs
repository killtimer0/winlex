use crate::{
    Architecture, Catalog,
    test_support::{source, type_id},
};
use windows_metadata::{reader, writer, *};

fn holder(file: &mut writer::File, ty: &Type) {
    let base = file.TypeRef("System", "ValueType");
    file.TypeDef(
        "Fixture",
        "Holder",
        writer::TypeDefOrRef::TypeRef(base),
        TypeAttributes::Public | TypeAttributes::SequentialLayout | TypeAttributes::WindowsRuntime,
    );
    file.Field("prefix", &Type::U8, FieldAttributes::Public);
    file.Field("value", ty, FieldAttributes::Public);
    file.Field("suffix", &Type::U8, FieldAttributes::Public);
}

#[test]
fn guid_values_and_arrays_have_native_layout_without_corelib() {
    for (ty, size, suffix, total) in [
        (Type::value_named("System", "Guid"), 128, 160, 192),
        (
            Type::ArrayFixed(Box::new(Type::value_named("System", "Guid")), 2),
            256,
            288,
            320,
        ),
    ] {
        let mut file = writer::File::new("NativeGuid");
        holder(&mut file, &ty);
        let db = Catalog::from_inputs([source("NativeGuid", file)]).unwrap();
        for arch in Architecture::ALL {
            let layout = db.layout(type_id(&db, "Holder"), arch).unwrap();
            assert_eq!((layout.size_bits, layout.alignment_bits), (total, 32));
            assert_eq!(layout.fields[1].offset_bits, 32);
            assert_eq!(layout.fields[1].size_bits, size);
            assert!(layout.fields[1].fields.is_empty());
            assert_eq!(layout.fields[2].offset_bits, suffix);
        }
    }
}

#[test]
fn foundation_names_do_not_invent_value_layouts_in_other_contexts() {
    for ty in [
        Type::class_named("System", "Guid"),
        Type::value_named("System", "Object"),
        Type::value_named("System", "ValueType"),
        Type::value_named("System", "Enum"),
        Type::value_named("System", "MulticastDelegate"),
        Type::value_named("System", "Attribute"),
        Type::value_named("System", "Type"),
    ] {
        let mut file = writer::File::new("InvalidFoundationValue");
        holder(&mut file, &ty);
        let db = Catalog::from_inputs([source("InvalidFoundationValue", file)]).unwrap();
        let error = db
            .layout(type_id(&db, "Holder"), Architecture::X64)
            .unwrap_err();
        assert!(error.to_string().contains("foundation type"), "{error:#}");
    }
}

#[test]
fn same_named_external_types_use_their_real_definitions_or_fail() {
    for namespace in ["System", "Other"] {
        let mut definition = writer::File::new("UserTypes");
        let base = definition.TypeRef("System", "ValueType");
        definition.TypeDef(
            namespace,
            "Guid",
            writer::TypeDefOrRef::TypeRef(base),
            TypeAttributes::Public
                | TypeAttributes::SequentialLayout
                | TypeAttributes::WindowsRuntime,
        );
        definition.Field("data", &Type::U64, FieldAttributes::Public);
        let definition = source("UserTypes", definition);
        let mut consumer = writer::File::new("Consumer");
        consumer.set_reference(reader::Index::new(vec![
            reader::File::new_shared(definition.bytes.clone()).unwrap(),
        ]));
        holder(&mut consumer, &Type::value_named(namespace, "Guid"));
        let consumer = source("Consumer", consumer);
        let missing = Catalog::from_inputs([consumer.clone()]).unwrap();
        let error = missing
            .layout(type_id(&missing, "Holder"), Architecture::X64)
            .unwrap_err();
        assert!(!error.to_string().contains("foundation type"));
        assert!(
            error.to_string().contains("matching definitions"),
            "{error:#}"
        );
        let db = Catalog::from_inputs([consumer, definition]).unwrap();
        for arch in Architecture::ALL {
            let layout = db.layout(type_id(&db, "Holder"), arch).unwrap();
            assert_eq!(layout.fields[1].size_bits, 64);
            assert_eq!(layout.fields[1].fields[0].name, "data");
        }
    }
}
