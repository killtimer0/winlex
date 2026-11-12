use serde_json::json;
use winlex_core::{Architecture, Bitfield, EntityId, FieldLayout, Layout, Slot, Vtable};

include!("support/source.rs");

#[test]
fn reference_json_distinguishes_builtin_definition_and_missing_dependency() {
    use winlex_core::{
        Catalog,
        windows_metadata::{TypeAttributes, reader, writer},
    };
    let mut file = writer::File::new("ReferenceSemantics");
    file.TypeRef("System", "Guid");
    file.TypeRef("System", "UnknownFoundationType");
    file.TypeRef("Fixture", "Present");
    file.TypeDef(
        "Fixture",
        "Present",
        writer::TypeDefOrRef::default(),
        TypeAttributes::Public,
    );
    let bytes = file.into_stream();
    let index = reader::Index::new(vec![reader::File::new(bytes.clone()).unwrap()]);
    let catalog = Catalog::from_inputs([source("reference-semantics", bytes)]).unwrap();
    for r in index.rows::<reader::TypeRef>(0) {
        let id = winlex_core::metadata::key(r, 1);
        let result =
            winlex_format::describe_target(&catalog, &id.into(), Some(Architecture::X64)).unwrap();
        let resolution = &result["resolution"];
        match r.name() {
            "Guid" => {
                assert_eq!(resolution["builtin"], "Guid");
                assert!(resolution.get("unresolved").is_none());
                assert!(resolution.get("target_id").is_none());
                assert!(catalog.resolve_reference(id, Architecture::X64).is_err());
            }
            "Present" => assert!(resolution["target_id"].is_object()),
            "UnknownFoundationType" => {
                assert!(resolution["unresolved"].is_string());
                assert!(resolution.get("builtin").is_none());
            }
            _ => unreachable!(),
        }
    }
}

const TYPE: EntityId = EntityId {
    source_id: 2,
    token: 0x02000001,
};
const FIELD: EntityId = EntityId {
    source_id: 3,
    token: 0x04000001,
};

#[test]
fn layout_offsets_are_relative_to_the_immediate_container() {
    let mut input = Layout {
        id: TYPE,
        name: "Outer".into(),
        architecture: Architecture::X64,
        size_bits: 128,
        alignment_bits: 32,
        dependencies_loaded: 2,
        fields: vec![FieldLayout {
            id: FIELD,
            name: "inner".into(),
            offset_bits: 64,
            size_bits: 64,
            bitfields: vec![],
            fields: vec![FieldLayout {
                id: FIELD,
                name: "flags".into(),
                offset_bits: 32,
                size_bits: 32,
                bitfields: vec![
                    Bitfield {
                        name: "first".into(),
                        relative_offset_bits: 0,
                        width_bits: 3,
                    },
                    Bitfield {
                        name: "second".into(),
                        relative_offset_bits: 3,
                        width_bits: 5,
                    },
                ],
                fields: vec![],
            }],
        }],
    };
    let mut expected = json!({
        "id": {"source_id": 2, "token": 0x02000001},
        "name": "Outer", "architecture": "x64",
        "size_bits": 128, "alignment_bits": 32, "dependencies_loaded": 2,
        "backend": "repc 0.1.1",
        "scope": "sequential and all-zero explicit overlays",
        "fields": [{
            "id": {"source_id": 3, "token": 0x04000001},
            "name": "inner", "offset_bits": 64, "size_bits": 64,
            "bitfields": [],
            "fields": [{
                "id": {"source_id": 3, "token": 0x04000001},
                "name": "flags", "offset_bits": 32, "size_bits": 32,
                "bitfields": [
                    {"name": "first", "relative_offset_bits": 0,
                     "width_bits": 3, "offset_bits": 32},
                    {"name": "second", "relative_offset_bits": 3,
                     "width_bits": 5, "offset_bits": 35}
                ]
            }]
        }]
    });
    assert_eq!(winlex_format::layout(&input), expected);
    input.fields.clear();
    expected["fields"] = json!([]);
    assert_eq!(winlex_format::layout(&input), expected);
}

#[test]
fn vtable_json_preserves_provenance_and_architecture_sized_slots() {
    for (architecture, name, stride) in [
        (Architecture::X86, "x86", 4),
        (Architecture::X64, "x64", 8),
        (Architecture::Arm64, "arm64", 8),
    ] {
        for windows_runtime in [false, true] {
            let mut input = Vtable {
                id: TYPE,
                name: "Interface".into(),
                architecture,
                windows_runtime,
                instance: None,
                slots: vec![
                    Slot {
                        method: "QueryInterface".into(),
                        declaring_type: "IUnknown".into(),
                        id: None,
                        declaring_type_id: None,
                        abi: None,
                    },
                    Slot {
                        method: "Method".into(),
                        declaring_type: "Base".into(),
                        id: Some(FIELD),
                        declaring_type_id: Some(TYPE),
                        abi: None,
                    },
                ],
            };
            let mut expected = json!({
                "id": {"source_id": 2, "token": 0x02000001},
                "name": "Interface", "architecture": name,
                "slots": [
                    {"method": "QueryInterface", "declaring_type": "IUnknown",
                     "id": null, "declaring_type_id": null,
                     "index": 0, "byte_offset": 0, "implicit": true},
                    {"method": "Method", "declaring_type": "Base",
                     "id": {"source_id": 3, "token": 0x04000001},
                     "declaring_type_id": {"source_id": 2, "token": 0x02000001},
                     "index": 1, "byte_offset": stride, "implicit": false}
                ]
            });
            if windows_runtime {
                expected["abi"] = json!("winrt");
                expected["scope"] =
                    json!("method slots only; metadata signatures are not lowered ABI signatures");
            }
            assert_eq!(winlex_format::vtable(&input), expected);
            if windows_runtime {
                input.instance = Some(winlex_core::InterfaceIdentity {
                    iid: "fixture-iid".into(),
                    signature: "fixture-signature".into(),
                });
                for abi in [
                    Ok(winlex_core::AbiMethod {
                        calling_convention: "platform",
                        return_type: "HRESULT",
                        parameters: vec![],
                    }),
                    Err("unsupported fixture type".into()),
                ] {
                    let mut method = json!({ "id": FIELD, "slot": 1, "offset_bytes": stride });
                    match &abi {
                        Ok(abi) => method["abi"] = json!(abi),
                        Err(reason) => method["unsupported"] = json!(reason),
                    }
                    input.slots[1].abi = Some(abi);
                    let actual = winlex_format::vtable(&input);
                    assert_eq!(actual["slots"], expected["slots"]);
                    assert_eq!(
                        actual["instance"],
                        json!({ "identity": {
                        "iid": "fixture-iid", "signature": "fixture-signature"
                    }, "methods": [method] })
                    );
                }
                input.instance = None;
            }
            input.slots.clear();
            expected["slots"] = json!([]);
            assert_eq!(winlex_format::vtable(&input), expected);
        }
    }
}
