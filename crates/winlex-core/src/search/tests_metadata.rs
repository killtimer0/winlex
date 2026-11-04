use super::*;
use crate::test_support::{FixtureFile, catalog, source, type_id};
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
    let constructor = file.constructor(attribute, values.iter().map(Value::ty).collect());
    let outer = file.public_type("Fixture", "Outer", object);
    for (name, value) in [
        ("257", Value::U16(257)),
        ("ValueOnly", Value::I32(257)),
        ("Fuzzy257", Value::U8(1)),
        ("Far", Value::F64(1.51)),
        ("Center", Value::F64(1.5)),
        ("Lower", Value::F64(1450.0)),
        ("Upper", Value::F64(1550.0)),
    ] {
        let field = file.constant(name, &value);
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
    let guid = file.public_type("Fixture", "Guid257", object);
    file.Attribute(
        writer::HasAttribute::TypeDef(guid),
        writer::AttributeType::MemberRef(constructor),
        &arguments,
    );
    file.public_type("Fixture.Other", "Child", object);
    file.public_type("FixtureExtra", "Excluded", object);
    source("SearchFixture", file)
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

#[test]
fn quick_and_conditional_trees_preserve_quality_and_evidence_order() {
    let db = Catalog::from_inputs([fixture(), conditions_fixture()]).unwrap();
    for (quick, mode, condition) in [
        ("257", SearchMode::Value, "value = 257"),
        ("1.5", SearchMode::Value, "value = 1.5"),
        ("NaN", SearchMode::Value, "value = NaN"),
        ("12345678", SearchMode::Guid, "guid contains \"12345678\""),
        (
            "Fixture.",
            SearchMode::Name,
            "fullname starts_with \"Fixture.\"",
        ),
    ] {
        let mut quick = hits(&db, quick, mode);
        let mut conditional = hits(&db, condition, SearchMode::Condition);
        assert!(!quick.is_empty());
        quick.sort_unstable_by_key(|hit| hit.id);
        conditional.sort_unstable_by_key(|hit| hit.id);
        assert_eq!(quick.len(), conditional.len());
        for (quick, conditional) in quick.iter().zip(&conditional) {
            assert_eq!(quick.id, conditional.id);
            assert_eq!(quick.evidence, conditional.evidence);
            assert_ne!(quick.quality, MatchQuality::Filter);
            if mode != SearchMode::Name {
                assert_eq!(conditional.quality, MatchQuality::Filter);
            }
        }
    }
    let combined = hits(&db, "257", SearchMode::Combined);
    let hit = combined.iter().find(|hit| hit.name == "257").unwrap();
    assert_eq!(hit.quality, MatchQuality::NameExact(MatchField::Name));
    assert_eq!(
        hit.evidence,
        [
            MatchEvidence {
                field: EvidenceField::Name,
                indices: Some(vec![0, 1, 2])
            },
            MatchEvidence {
                field: EvidenceField::Value,
                indices: None
            },
        ]
    );
    let filtered = hits(&db, "name = \"257\" and value = 257", SearchMode::Condition);
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].quality, MatchQuality::Filter);
    assert_eq!(filtered[0].evidence, hit.evidence);
}

#[test]
fn quick_and_conditional_text_keep_original_unicode_evidence() {
    let mut file = writer::File::new("Unicode");
    let object = file.TypeRef("System", "Object");
    file.public_type("İScope", "İ😀Widget", object);
    let db = catalog("Unicode", file);
    for (quick, condition, expected) in [
        ("i\u{307}😀widget", "name = \"i\u{307}😀widget\"", 0..8),
        (
            "i\u{307}scope.",
            "fullname starts_with \"i\u{307}scope.\"",
            0..7,
        ),
    ] {
        let quick = hits(&db, quick, SearchMode::Name);
        let conditional = hits(&db, condition, SearchMode::Condition);
        assert_eq!(quick.len(), 1);
        assert_eq!(conditional.len(), 1);
        assert_eq!(quick[0].id, conditional[0].id);
        assert_eq!(quick[0].evidence, conditional[0].evidence);
        assert_eq!(quick[0].evidence[0].indices, Some(expected.collect()));
    }
}

fn conditions_fixture() -> SourceData {
    let mut file = writer::File::new("Conditions");
    let object = file.TypeRef("System", "Object");
    file.public_type("Example", "Constants", object);
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
        file.constant(name, &value);
    }
    source("Conditions", file)
}

#[test]
fn category_vocabulary_preserves_bits_summary_kinds_and_condition_parsing() {
    let mut file = writer::File::new("Categories");
    for (name, base) in [
        ("Class", "Object"),
        ("Attribute", "Attribute"),
        ("Struct", "ValueType"),
        ("Enum", "Enum"),
        ("Delegate", "MulticastDelegate"),
    ] {
        let base = file.TypeRef("System", base);
        file.public_type("Fixture", name, base);
    }
    let interface = file.TypeDef(
        "Fixture",
        "Interface",
        writer::TypeDefOrRef::default(),
        TypeAttributes::Public,
    );
    file.Field("Field", &Type::I32, FieldAttributes::Public);
    file.constant("Constant", &Value::I32(1));
    file.MethodDef(
        "Method",
        &Signature {
            flags: MethodCallAttributes::default(),
            return_type: Type::Void,
            types: vec![],
        },
        MethodAttributes::Public,
        MethodImplAttributes::default(),
    );
    let property = file.Property("Property", &Type::I32);
    file.PropertyMap(interface, property);
    let event = file.Event("Event", &Type::class_named("Fixture", "Delegate"));
    file.EventMap(interface, event);
    let db = catalog("Categories", file);
    assert_eq!(EntityCategory::ALL, 127);
    assert_eq!(EntityCategory::DEFAULT, 7);
    for (name, category, bit, spelling) in [
        ("Interface", EntityCategory::Type, 1, "type"),
        ("Method", EntityCategory::Method, 2, "method"),
        ("Constant", EntityCategory::Constant, 4, "constant"),
        ("Field", EntityCategory::Field, 8, "field"),
        ("Property", EntityCategory::Property, 16, "property"),
        ("Event", EntityCategory::Event, 32, "event"),
        (
            "Object",
            EntityCategory::TypeReference,
            64,
            "type_reference",
        ),
    ] {
        assert_eq!(category as u32, bit);
        let selected = hits(&db, name, SearchMode::Name).pop().unwrap();
        assert_eq!(
            db.declaration(selected.id, None).unwrap().category(),
            category
        );
        if category != EntityCategory::Type {
            assert_eq!(selected.kind, spelling);
        }
        let expected: Vec<_> = db
            .search(
                "not name = \"missing\"",
                SearchOptions {
                    mode: SearchMode::Condition,
                    categories: Some(bit),
                    ..Default::default()
                },
            )
            .unwrap()
            .map(|hit| hit.unwrap().id)
            .collect();
        for text in [spelling.to_owned(), spelling.to_ascii_uppercase()] {
            let mut found = hits(&db, &format!("category = {text:?}"), SearchMode::Condition);
            found.sort_unstable_by_key(|hit| hit.id);
            assert_eq!(found.iter().map(|hit| hit.id).collect::<Vec<_>>(), expected);
            assert!(expected.contains(&selected.id));
        }
    }
    for name in [
        "<Module>",
        "Class",
        "Attribute",
        "Struct",
        "Enum",
        "Delegate",
        "Interface",
    ] {
        let selected = hits(&db, name, SearchMode::Name).pop().unwrap();
        assert_eq!(
            selected.kind,
            name.trim_matches(['<', '>']).to_ascii_lowercase()
        );
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
fn text_operator_matrix_preserves_acceptance_and_diagnostic_precedence() {
    use Operator::*;
    for (field, accepted) in [
        ("name", vec![Equal, Contains, StartsWith, Fuzzy]),
        ("fullname", vec![Equal, Contains, StartsWith, Fuzzy]),
        ("value", vec![Equal, Contains, StartsWith]),
        ("namespace", vec![Equal, Within]),
    ] {
        for operator in [
            Equal,
            Less,
            LessEqual,
            Greater,
            GreaterEqual,
            Contains,
            StartsWith,
            Fuzzy,
            Within,
        ] {
            for value in [
                Literal::Text("".into()),
                Literal::Text("窗口😀".into()),
                Literal::Number("5".into()),
            ] {
                let query = Query::Predicate(Predicate {
                    field: field.into(),
                    operator,
                    value: value.clone(),
                    span: 7..29,
                });
                let result = SearchQuery::from_query(&query, false);
                let quoted = matches!(value, Literal::Text(_));
                if (quoted && accepted.contains(&operator))
                    || (field == "value"
                        && !quoted
                        && [Equal, Less, LessEqual, Greater, GreaterEqual].contains(&operator))
                {
                    assert!(result.is_ok(), "{query:?}");
                } else {
                    let error = result.err().unwrap();
                    let diagnostic = error.downcast_ref::<QueryError>().unwrap();
                    assert_eq!(diagnostic.span, 7..29);
                    assert!(!diagnostic.incomplete);
                    if !quoted && field != "value" {
                        assert_eq!(diagnostic.kind, QueryErrorKind::Value);
                        assert_eq!(diagnostic.message, "field requires a quoted string");
                    } else {
                        assert_eq!(diagnostic.kind, QueryErrorKind::Operator);
                        assert_eq!(
                            diagnostic.message,
                            if quoted {
                                "operator is not supported for this field"
                            } else {
                                "number requires a comparison operator"
                            }
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn numeric_conditions_keep_filter_ranking_for_each_operator_and_group() {
    let db = Catalog::from_inputs([conditions_fixture()]).unwrap();
    for bound in ["5", "5.0"] {
        for op in ["=", "<", "<=", ">", ">="] {
            let predicate = format!("value {op} {bound}");
            for query in [
                predicate.clone(),
                format!("({predicate}) or name = \"ACCESS_DENIED\""),
                format!("({predicate}) and category = \"constant\""),
                format!("not not ({predicate})"),
            ] {
                let found = hits(&db, &query, SearchMode::Condition);
                if query.contains(" or ") {
                    assert!(!found.is_empty(), "{query}");
                }
                assert!(
                    found.iter().all(|hit| hit.quality == MatchQuality::Filter),
                    "{query}"
                );
            }
        }
    }
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
fn condition_diagnostics_keep_field_operator_and_value_errors_distinct() {
    use super::syntax::QueryErrorKind;
    for (query, kind) in [
        ("unknown = 5", QueryErrorKind::Field),
        (r#"unknown = "text""#, QueryErrorKind::Field),
        (r#"name > "text""#, QueryErrorKind::Operator),
        ("name > 5", QueryErrorKind::Value),
        ("value contains 5", QueryErrorKind::Operator),
        (r#"source contains "text""#, QueryErrorKind::Operator),
        (r#"source = "0""#, QueryErrorKind::Value),
        (r#"category = "unknown""#, QueryErrorKind::Value),
        ("architecture = 5", QueryErrorKind::Value),
    ] {
        let parsed = Query::parse(query).unwrap();
        let error = SearchQuery::from_query(&parsed, false).err().unwrap();
        let diagnostic = error.downcast_ref::<QueryError>().unwrap();
        assert_eq!(diagnostic.kind, kind, "{query}");
        assert_eq!(diagnostic.span, 0..query.len(), "{query}");
        if query == r#"category = "unknown""# {
            assert_eq!(diagnostic.message, "unknown category");
            assert!(!diagnostic.incomplete);
        }
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
fn search_entry_points_preserve_scan_order_quality_evidence_and_filters() {
    let db = Catalog::from_inputs([fixture(), conditions_fixture()]).unwrap();
    let snapshot = |search: crate::Search<'_>| {
        format!("{:?}", search.collect::<anyhow::Result<Vec<_>>>().unwrap())
    };
    for (text, mode) in [
        ("257", SearchMode::Combined),
        ("Fixture.", SearchMode::Name),
        ("257", SearchMode::Value),
        ("12345678", SearchMode::Guid),
        (
            r#"value = 5 or value contains "Windows""#,
            SearchMode::Condition,
        ),
    ] {
        for source in [None, Some(0), Some(1)] {
            for categories in [None, Some(EntityCategory::Constant as u32)] {
                let options = || SearchOptions {
                    source,
                    categories,
                    mode,
                    architecture: Some(crate::Architecture::X86),
                    ..Default::default()
                };
                let direct = snapshot(db.search(text, options()).unwrap());
                let compiled = SearchQuery::compile(text, mode, false).unwrap();
                assert_eq!(
                    direct,
                    snapshot(db.search_compiled(compiled, options()).unwrap())
                );
                if mode == SearchMode::Condition {
                    let parsed = Query::parse(text).unwrap();
                    let compiled = SearchQuery::from_query(&parsed, false).unwrap();
                    assert_eq!(
                        direct,
                        snapshot(db.search_compiled(compiled, options()).unwrap())
                    );
                }
            }
        }
    }
}

#[test]
fn search_entry_points_preserve_input_diagnostic_wrappers() {
    let db = Catalog::from_inputs([fixture()]).unwrap();
    for text in ["", "value =", "unknown = 1"] {
        let error = db
            .search(
                text,
                SearchOptions {
                    mode: SearchMode::Condition,
                    ..Default::default()
                },
            )
            .err()
            .unwrap();
        assert_eq!(
            error.downcast_ref::<crate::InvalidSearchInput>().is_some(),
            !text.is_empty()
        );
        if text.is_empty() {
            assert_eq!(error.to_string(), "empty search query");
        } else {
            assert!(error.downcast_ref::<QueryError>().is_some());
        }
    }
    let parsed = Query::parse("unknown = 1").unwrap();
    let error = SearchQuery::from_query(&parsed, false).err().unwrap();
    assert!(error.downcast_ref::<crate::InvalidSearchInput>().is_none());
    assert_eq!(
        error.downcast_ref::<QueryError>().unwrap().kind,
        QueryErrorKind::Field
    );
}

#[test]
fn compiled_search_applies_external_source_filter() {
    let db = Catalog::from_inputs([fixture(), fixture()]).unwrap();
    let compiled = SearchQuery::compile("ValueOnly", SearchMode::Name, false).unwrap();
    let found = db
        .search_compiled(
            compiled,
            SearchOptions {
                source: Some(1),
                ..Default::default()
            },
        )
        .unwrap()
        .collect::<anyhow::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].id.source_id, 1);
    assert_eq!(found[0].full_name, "Fixture.Outer.ValueOnly");
    assert_eq!(found[0].quality, MatchQuality::NameExact(MatchField::Name));
}

#[test]
fn namespace_filters_and_entity_context_agree_for_nested_and_global_declarations() {
    let mut file = writer::File::new("NamespaceFixture");
    let object = file.TypeRef("System", "Object");
    file.TypeRef("", "GlobalReference");
    for namespace in ["", "命名.空间"] {
        let outer = file.public_type(namespace, "Outer", object);
        file.Field("TopMember", &Type::I32, FieldAttributes::Public);
        let inner = file.TypeDef(
            "",
            "Inner",
            writer::TypeDefOrRef::TypeRef(object),
            TypeAttributes::NestedPublic,
        );
        file.NestedClass(inner, outer);
        file.Field("NestedMember", &Type::I32, FieldAttributes::Public);
    }
    let db = catalog("NamespaceFixture", file);
    let mut covered = 0;
    for namespace in ["", "命名.空间", "System"] {
        let found = db
            .search(
                &format!("namespace = {namespace:?}"),
                SearchOptions {
                    mode: SearchMode::Condition,
                    categories: Some(EntityCategory::ALL),
                    ..Default::default()
                },
            )
            .unwrap()
            .collect::<anyhow::Result<Vec<_>>>()
            .unwrap();
        assert!(!found.is_empty(), "{namespace}");
        for hit in found {
            covered += 1;
            let context = db.entity_context(hit.id).unwrap();
            assert_eq!(context.namespace, namespace, "{}", hit.full_name);
            assert_eq!(
                context.owner.as_deref(),
                match hit.name.as_str() {
                    "TopMember" => Some("Outer"),
                    "NestedMember" => Some("Outer+Inner"),
                    _ => None,
                }
            );
        }
    }
    let all = db
        .search(
            "not namespace = \"missing\"",
            SearchOptions {
                mode: SearchMode::Condition,
                categories: Some(EntityCategory::ALL),
                ..Default::default()
            },
        )
        .unwrap()
        .collect::<anyhow::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(covered, all.len());
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
        let owner = file.public_type("Fixture", "Owner", object);
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
        let db = catalog("References", file);
        let target = type_id(&db, "Target");
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
