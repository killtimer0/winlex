use super::*;
use crate::{Catalog, EntityCategory, SearchOptions, SourceData};
use windows_metadata::{writer, *};

fn fixture() -> SourceData {
    let mut file = writer::File::new("SearchFixture");
    let object = file.TypeRef("System", "Object");
    let attribute = file.TypeRef("Windows.Foundation.Metadata", "GuidAttribute");
    let values = [
        Value::U32(0x12345678),
        Value::U16(0x9abc),
        Value::U16(0xdef0),
        Value::U8(0x12),
        Value::U8(0x34),
        Value::U8(0x56),
        Value::U8(0x78),
        Value::U8(0x9a),
        Value::U8(0xbc),
        Value::U8(0xde),
        Value::U8(0xf0),
    ];
    let constructor = file.MemberRef(
        ".ctor",
        &Signature {
            flags: MethodCallAttributes::HASTHIS,
            return_type: Type::Void,
            types: values.iter().map(Value::ty).collect(),
        },
        writer::MemberRefParent::TypeRef(attribute),
    );
    let outer = file.TypeDef(
        "Fixture",
        "Outer",
        writer::TypeDefOrRef::TypeRef(object),
        TypeAttributes::Public,
    );
    for (name, value) in [
        ("257", Value::U16(257)),
        ("ValueOnly", Value::I32(257)),
        ("Fuzzy257", Value::U8(1)),
        ("Far", Value::F64(1.51)),
        ("Center", Value::F64(1.5)),
        ("Lower", Value::F64(1450.0)),
        ("Upper", Value::F64(1550.0)),
    ] {
        let field = file.Field(
            name,
            &value.ty(),
            FieldAttributes::Public | FieldAttributes::Static | FieldAttributes::Literal,
        );
        file.Constant(writer::HasConstant::Field(field), &value);
        if name == "257" || name == "ValueOnly" {
            let mut arguments: Vec<_> = values
                .iter()
                .cloned()
                .map(|value| (String::new(), value))
                .collect();
            arguments[0].1 = Value::U32(0x02570000);
            file.Attribute(
                writer::HasAttribute::Field(field),
                writer::AttributeType::MemberRef(constructor),
                &arguments,
            );
        }
    }
    let field = file.Field(
        "GuidField",
        &Type::U32,
        FieldAttributes::Public | FieldAttributes::Static,
    );
    let arguments: Vec<_> = values
        .into_iter()
        .map(|value| (String::new(), value))
        .collect();
    file.Attribute(
        writer::HasAttribute::Field(field),
        writer::AttributeType::MemberRef(constructor),
        &arguments,
    );
    let inner = file.TypeDef(
        "",
        "Inner",
        writer::TypeDefOrRef::TypeRef(object),
        TypeAttributes::NestedPublic,
    );
    file.NestedClass(inner, outer);
    let guid = file.TypeDef(
        "Fixture",
        "Guid257",
        writer::TypeDefOrRef::TypeRef(object),
        TypeAttributes::Public,
    );
    file.Attribute(
        writer::HasAttribute::TypeDef(guid),
        writer::AttributeType::MemberRef(constructor),
        &arguments,
    );
    file.TypeDef(
        "Fixture.Other",
        "Child",
        writer::TypeDefOrRef::TypeRef(object),
        TypeAttributes::Public,
    );
    file.TypeDef(
        "FixtureExtra",
        "Excluded",
        writer::TypeDefOrRef::TypeRef(object),
        TypeAttributes::Public,
    );
    SourceData {
        path: "SearchFixture.winmd".into(),
        bytes: file.into_stream().into(),
    }
}

fn hits(db: &Catalog, query: &str, mode: SearchMode) -> Vec<SearchMatch> {
    let mut hits = db
        .search(
            query,
            SearchOptions {
                mode,
                ..Default::default()
            },
        )
        .unwrap()
        .collect::<anyhow::Result<Vec<_>>>()
        .unwrap();
    hits.sort_unstable_by(SearchMatch::compare);
    hits
}

fn conditions_fixture() -> SourceData {
    let mut file = writer::File::new("Conditions");
    let object = file.TypeRef("System", "Object");
    file.TypeDef(
        "Example",
        "Constants",
        writer::TypeDefOrRef::TypeRef(object),
        TypeAttributes::Public,
    );
    for (name, value) in [
        ("ACCESS_DENIED", Value::I32(5)),
        ("ACCESS_OTHER", Value::I32(6)),
        ("OTHER_FIVE", Value::U32(5)),
        ("SERVICE_TEXT", Value::Utf16("Windows\n😀\"\\".into())),
        ("STRING_FIVE", Value::Utf16("5".into())),
        ("EMPTY", Value::Utf16("".into())),
        ("FLOAT_FIVE", Value::F32(5.0)),
        ("NAN_VALUE", Value::F64(f64::NAN)),
        ("POS_INF", Value::F64(f64::INFINITY)),
        ("NEG_INF", Value::F32(f32::NEG_INFINITY)),
    ] {
        let field = file.Field(
            name,
            &value.ty(),
            FieldAttributes::Public | FieldAttributes::Static | FieldAttributes::Literal,
        );
        file.Constant(writer::HasConstant::Field(field), &value);
    }
    SourceData {
        path: "Conditions.winmd".into(),
        bytes: file.into_stream().into(),
    }
}

#[test]
fn conditions_combine_same_entity_fields_and_keep_all_positive_evidence() {
    let db = Catalog::from_inputs([conditions_fixture()]).unwrap();
    let query = r#"name contains "access" and value = 5"#;
    let found = hits(&db, query, SearchMode::Condition);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].name, "ACCESS_DENIED");
    assert_eq!(found[0].evidence.len(), 2);
    assert_eq!(found[0].evidence[0].field, EvidenceField::Name);
    assert_eq!(found[0].evidence[1].field, EvidenceField::Value);
    assert_eq!(
        found[0].id,
        hits(
            &db,
            r#"value = 5 and name contains "access""#,
            SearchMode::Condition
        )[0]
        .id
    );
    let parsed = Query::parse(query).unwrap();
    assert_eq!(
        db.search_query(&parsed, SearchOptions::default())
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .id,
        found[0].id
    );
    assert_eq!(
        hits(
            &db,
            r#"name = "ACCESS_DENIED" or value = 5 and name = "OTHER_FIVE""#,
            SearchMode::Condition
        )
        .len(),
        2
    );
    assert_eq!(
        hits(
            &db,
            r#"(name = "ACCESS_DENIED" or value = 5) and name = "OTHER_FIVE""#,
            SearchMode::Condition
        )
        .len(),
        1
    );
    let complement = hits(&db, "not (value = 5)", SearchMode::Condition);
    assert!(complement.iter().any(|r| r.name == "Constants"));
    assert!(complement.iter().all(|r| r.evidence.is_empty()));
    assert!(
        db.search(
            query,
            SearchOptions {
                mode: SearchMode::Condition,
                categories: Some(EntityCategory::Type as u32),
                ..Default::default()
            }
        )
        .unwrap()
        .next()
        .is_none()
    );
}

#[test]
fn strings_special_floats_and_ranges_are_typed_conditions() {
    let db = Catalog::from_inputs([conditions_fixture()]).unwrap();
    for (query, expected) in [
        (
            r#"name contains "SERVICE" and value contains "Windows""#,
            vec!["SERVICE_TEXT"],
        ),
        (r#"value = "Windows\n😀\"\\""#, vec!["SERVICE_TEXT"]),
        (r#"value = """#, vec!["EMPTY"]),
        (r#"value = "5""#, vec!["STRING_FIVE"]),
        ("value = 5.0", vec!["FLOAT_FIVE"]),
        ("value = nan", vec!["NAN_VALUE"]),
        ("value = inf", vec!["POS_INF"]),
        ("value = -inf", vec!["NEG_INF"]),
        ("value > -inf and value < inf", vec!["FLOAT_FIVE"]),
        ("value > 5 and value <= 6", vec!["ACCESS_OTHER"]),
    ] {
        let found = hits(&db, query, SearchMode::Condition);
        assert_eq!(
            found.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(),
            expected,
            "{query}"
        );
    }
    assert_eq!(
        hits(&db, "Windows", SearchMode::Combined)[0].name,
        "SERVICE_TEXT"
    );
    assert!(
        hits(&db, "5", SearchMode::Combined)
            .iter()
            .any(|r| r.name == "STRING_FIVE")
    );
    for query in [
        "value < nan",
        "value contains 5",
        "value = 1e9999",
        "unknown = 5",
        "source = -1",
    ] {
        let error = db
            .search(
                query,
                SearchOptions {
                    mode: SearchMode::Condition,
                    ..Default::default()
                },
            )
            .err()
            .unwrap();
        assert!(error.downcast_ref::<QueryError>().is_some(), "{error:#}");
    }
}

#[test]
fn condition_scopes_and_sources_intersect_external_filters() {
    let db = Catalog::from_inputs([fixture(), fixture()]).unwrap();
    let query = r#"namespace within "Fixture" and category = "type" and source = 1 and architecture = "x86""#;
    let found = hits(&db, query, SearchMode::Condition);
    assert!(!found.is_empty());
    assert!(
        found
            .iter()
            .all(|r| r.id.source_id == 1 && r.name != "Excluded")
    );
    assert!(found.iter().any(|r| r.name == "Child"));
    let exact = hits(
        &db,
        r#"namespace = "Fixture" and category = "type""#,
        SearchMode::Condition,
    );
    assert!(!exact.iter().any(|r| r.name == "Child"));
    assert!(
        db.search(
            query,
            SearchOptions {
                mode: SearchMode::Condition,
                source: Some(0),
                ..Default::default()
            }
        )
        .unwrap()
        .next()
        .is_none()
    );
    assert!(
        hits(
            &db,
            r#"guid = "12345678-9abc-def0-1234-56789abcdef0""#,
            SearchMode::Condition
        )
        .len()
            >= 2
    );
}

#[test]
fn combined_union_is_unique_and_uses_strongest_quality() {
    let db = Catalog::from_inputs([fixture()]).unwrap();
    let combined = hits(&db, "257", SearchMode::Combined);
    assert_eq!(combined.iter().filter(|hit| hit.name == "257").count(), 1);
    assert!(matches!(combined[0].quality, MatchQuality::NameExact(_)));
    assert_eq!(combined[1].name, "ValueOnly");
    assert_eq!(combined[1].quality, MatchQuality::Integer);
    assert_eq!(hits(&db, "257", SearchMode::Guid).len(), 2);
    let mut ids: Vec<_> = combined.iter().map(|hit| hit.id).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), combined.len());
    assert!(combined.iter().any(|hit| hit.name == "Fuzzy257"));
    let names = hits(&db, "257", SearchMode::Name);
    assert_eq!(names.len(), 1);
    assert_eq!(hits(&db, "257", SearchMode::Value).len(), 2);
    let guid = hits(&db, "12345678", SearchMode::Combined);
    assert_eq!(guid.len(), 4);
    assert!(
        guid.iter()
            .all(|hit| hit.quality == MatchQuality::GuidFragment)
    );
    assert_eq!(hits(&db, "not-a-number", SearchMode::Combined).len(), 0);
    assert!(
        db.search(
            "not-a-number",
            SearchOptions {
                mode: SearchMode::Value,
                ..Default::default()
            }
        )
        .is_err()
    );
}

#[test]
fn entity_context_preserves_metadata_namespace_owner_and_values() {
    let db = Catalog::from_inputs([fixture()]).unwrap();
    let field = hits(&db, "ValueOnly", SearchMode::Name).pop().unwrap();
    let context = db.entity_context(field.id).unwrap();
    assert_eq!(context.namespace, "Fixture");
    assert_eq!(context.owner.as_deref(), Some("Outer"));
    assert!(matches!(context.constant, Some(Value::I32(257))));
    assert_eq!(
        context.guid.as_deref(),
        Some("02570000-9abc-def0-1234-56789abcdef0")
    );
    let inner = hits(&db, "Fixture.Outer+Inner", SearchMode::Name)
        .pop()
        .unwrap();
    let context = db.entity_context(inner.id).unwrap();
    assert_eq!(context.namespace, "Fixture");
    assert!(context.owner.is_none());
    assert!(context.constant.is_none());
}

#[test]
fn combined_guid_matching_requires_four_normalized_hex_digits() {
    let db = Catalog::from_inputs([fixture()]).unwrap();
    for query in ["5", "bc", "abc", "{abc}", "0-1", "0xBC", "123"] {
        let combined = hits(&db, query, SearchMode::Combined);
        assert!(
            combined.iter().all(|hit| !matches!(
                hit.quality,
                MatchQuality::GuidFragment | MatchQuality::GuidExact
            )),
            "{query}"
        );
        assert!(!hits(&db, query, SearchMode::Guid).is_empty(), "{query}");
    }
    for query in [
        "9abc",
        "{9ABC}",
        "f0-12",
        "120,154",
        "78 9A",
        "0xBC,0xDE",
        "12345678-9abc-def0-1234-56789abcdef0",
    ] {
        let combined = hits(&db, query, SearchMode::Combined);
        let guid = hits(&db, query, SearchMode::Guid);
        assert!(!guid.is_empty(), "{query}");
        assert_eq!(
            combined.iter().map(|hit| hit.id).collect::<Vec<_>>(),
            guid.iter().map(|hit| hit.id).collect::<Vec<_>>(),
            "{query}"
        );
    }
    let combined = hits(&db, "257", SearchMode::Combined);
    assert!(
        combined
            .iter()
            .any(|hit| hit.quality == MatchQuality::Integer)
    );
    assert!(combined.iter().any(|hit| hit.name == "Fuzzy257"));
}

#[test]
fn scopes_respect_boundaries_and_strict_selectors_remain_independent() {
    let db = Catalog::from_inputs([fixture()]).unwrap();
    let scoped = hits(&db, "Fixture.", SearchMode::Name);
    assert!(scoped.iter().any(|hit| hit.name == "Child"));
    assert!(scoped.iter().all(|hit| hit.name != "Excluded"));
    let nested = hits(&db, "Fixture.Outer+", SearchMode::Name);
    assert_eq!(nested.len(), 1);
    assert_eq!(nested[0].name, "Inner");
    assert!(hits(&db, "Fixture.Out+", SearchMode::Name).is_empty());
    assert_eq!(hits(&db, "fixture.outer+inner", SearchMode::Name).len(), 1);
    for query in ["Fixture.", "Fixture.Outer+", "Fuzzy", "12345678"] {
        assert_eq!(
            db.search_exact(query, SearchOptions::default())
                .unwrap()
                .count(),
            0
        );
    }
}

#[test]
fn float_equality_guid_order_and_source_filters_use_shared_scan() {
    let db = Catalog::from_inputs([fixture(), fixture()]).unwrap();
    let floats = hits(&db, "1.5", SearchMode::Value);
    assert_eq!(
        floats
            .iter()
            .map(|hit| hit.name.as_str())
            .collect::<Vec<_>>(),
        ["Center", "Center"]
    );
    let interval = hits(&db, "1.5e3", SearchMode::Value);
    assert!(interval.is_empty());
    for query in [
        "12345678-9ABC-def0-1234-56789abcdef0",
        "120,154",
        "78 9A",
        "0x78,0x9a",
    ] {
        let expected = if query.starts_with("12345678") { 4 } else { 8 };
        assert_eq!(
            hits(&db, query, SearchMode::Guid).len(),
            expected,
            "{query}"
        );
    }
    assert!(hits(&db, "78563412", SearchMode::Guid).is_empty());
    let filtered = db
        .search(
            "12345678",
            SearchOptions {
                mode: SearchMode::Guid,
                source: Some(1),
                categories: Some(EntityCategory::Type as u32),
                ..Default::default()
            },
        )
        .unwrap()
        .collect::<anyhow::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].id.source_id, 1);
    assert_eq!(filtered[0].name, "Guid257");
}

#[test]
fn reference_counts_do_not_short_circuit_after_an_interface_hit() {
    for target_first in [false, true] {
        let mut file = writer::File::new("References");
        let object = file.TypeRef("System", "Object");
        file.TypeDef(
            "Fixture",
            "Target",
            writer::TypeDefOrRef::TypeRef(object),
            TypeAttributes::Public | TypeAttributes::Interface | TypeAttributes::Abstract,
        );
        let owner = file.TypeDef(
            "Fixture",
            "Owner",
            writer::TypeDefOrRef::TypeRef(object),
            TypeAttributes::Public,
        );
        let target = Type::ClassName(TypeName::named("Fixture", "Target"));
        let missing = Type::ClassName(TypeName::named("Fixture", "Missing"));
        let interfaces = if target_first {
            [&target, &missing]
        } else {
            [&missing, &target]
        };
        for interface in interfaces {
            file.InterfaceImpl(owner, interface);
        }
        let db = Catalog::from_inputs([SourceData {
            path: "References.winmd".into(),
            bytes: file.into_stream().into(),
        }])
        .unwrap();
        let target = db
            .search_exact(
                "Target",
                SearchOptions {
                    types_only: true,
                    ..Default::default()
                },
            )
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .id;
        let mut query = db.references(target, Architecture::X64, None).unwrap();
        let matches = query.by_ref().collect::<anyhow::Result<Vec<_>>>().unwrap();
        assert_eq!(
            matches
                .iter()
                .map(|hit| hit.name.as_str())
                .collect::<Vec<_>>(),
            ["Owner"]
        );
        assert_eq!(
            query.unresolved_type_uses(),
            1,
            "target_first={target_first}"
        );
    }
}
