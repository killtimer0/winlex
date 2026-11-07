use serde_json::json;
use winlex_core::{Architecture, Bitfield, EntityId, FieldLayout, Layout, Slot, Vtable};

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
                slots: vec![
                    Slot {
                        method: "QueryInterface".into(),
                        declaring_type: "IUnknown".into(),
                        id: None,
                        declaring_type_id: None,
                    },
                    Slot {
                        method: "Method".into(),
                        declaring_type: "Base".into(),
                        id: Some(FIELD),
                        declaring_type_id: Some(TYPE),
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
            input.slots.clear();
            expected["slots"] = json!([]);
            assert_eq!(winlex_format::vtable(&input), expected);
        }
    }
}
