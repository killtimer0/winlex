use winlex_core::windows_metadata::{writer, *};
use winlex_core::{Architecture, Catalog, Declaration, EntityCategory, SearchOptions};
use winlex_format::declaration::{RegionKind, render_target};

include!("support/source.rs");

fn find(catalog: &Catalog, name: &str) -> winlex_core::EntityId {
    catalog
        .search_exact(
            name,
            SearchOptions {
                categories: Some(EntityCategory::Type as u32),
                ..Default::default()
            },
        )
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .id
}

#[test]
fn instance_links_keep_arguments_across_members_and_nested_navigation() {
    let mut file = writer::File::new("InstancePresentation");
    let box_type = file.TypeDef(
        "Fixture",
        "Box`1",
        writer::TypeDefOrRef::default(),
        TypeAttributes::Public,
    );
    file.GenericParam(
        "T",
        writer::TypeOrMethodDef::TypeDef(box_type),
        0,
        GenericParamAttributes(0),
    );
    file.Field(
        "Value",
        &Type::Generic("T".into(), 0),
        FieldAttributes::Public,
    );
    let property = file.Property("Current", &Type::Generic("T".into(), 0));
    file.PropertyMap(box_type, property);
    let mut nested = TypeName::named("Fixture", "Box`1");
    nested.generics = vec![Type::Generic("T".into(), 0)];
    file.InterfaceImpl(box_type, &Type::ClassName(nested.clone()));
    nested.generics = vec![Type::ClassName(nested.clone())];
    file.MethodDef(
        "Nested",
        &Signature {
            flags: MethodCallAttributes::HASTHIS,
            return_type: Type::ClassName(nested),
            types: vec![],
        },
        MethodAttributes::Public,
        MethodImplAttributes(0),
    );
    file.TypeDef(
        "Fixture",
        "Uses",
        writer::TypeDefOrRef::default(),
        TypeAttributes::Public,
    );
    let mut name = TypeName::named("Fixture", "Box`1");
    name.generics = vec![Type::I32];
    file.Field("Integers", &Type::ClassName(name), FieldAttributes::Public);
    let catalog = Catalog::from_inputs([source("instances", file.into_stream())]).unwrap();
    let arch = Architecture::X64;
    let source = render_target(&catalog, &find(&catalog, "Uses").into(), arch).unwrap();
    let target = source
        .regions
        .iter()
        .find_map(|region| match &region.kind {
            RegionKind::Reference(target) if !target.arguments.is_empty() => Some(target.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(target.arguments, [Type::I32]);
    let instance = render_target(&catalog, &target, arch).unwrap();
    assert!(instance.text.contains("Box<int32>"));
    assert!(instance.text.contains("int32 Value;"));
    assert!(instance.text.contains("int32 Current;"));
    assert!(winlex_format::describe_target(&catalog, &target, None).is_err());
    let json = winlex_format::describe_target(&catalog, &target, Some(arch)).unwrap();
    assert_eq!(json["entity"]["name"], "Box<int32>");
    assert_eq!(json["fields"][0]["type"]["name"], "int32");
    assert_eq!(json["properties"][0]["type"]["name"], "int32");
    assert_eq!(
        json["interface_implementations"][0]["type"]["arguments"][0]["name"],
        "int32"
    );
    assert_eq!(
        json["methods"][0]["return_type"]["arguments"][0]["arguments"][0]["name"],
        "int32"
    );
    let nested = instance
        .regions
        .iter()
        .find_map(|region| match &region.kind {
            RegionKind::Reference(target)
                if matches!(target.arguments.first(), Some(Type::ClassName(_))) =>
            {
                Some(target)
            }
            _ => None,
        })
        .unwrap();
    let nested_view = render_target(&catalog, nested, arch).unwrap();
    assert!(nested_view.text.contains("Box<Box<int32>>"));
    assert!(nested_view.text.contains("Box<int32> Value;"));
    let Declaration::Type(row) = catalog.declaration(target.id, None).unwrap() else {
        panic!()
    };
    let field = winlex_core::metadata::key(row.fields().next().unwrap(), 4);
    assert!(
        render_target(&catalog, &target.with_id(field), arch)
            .unwrap()
            .text
            .contains("int32 Value;")
    );
    let definition = render_target(&catalog, &target.id.into(), arch).unwrap();
    assert!(definition.text.contains("T Value;"));
    assert!(
        definition
            .regions
            .iter()
            .filter_map(|r| match &r.kind {
                RegionKind::Reference(t) => Some(t),
                _ => None,
            })
            .all(|t| t.arguments.is_empty())
    );
}
