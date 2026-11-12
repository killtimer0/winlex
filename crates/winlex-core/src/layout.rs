//! Compute each named type once and reuse its complete ABI layout in containing records.
use crate::metadata::{
    Db, instance_fields, native_alignment, native_alignment_attributes, supports,
};
use crate::source::EntityId;
use crate::{Architecture, FieldLayout, Layout};
use anyhow::{Result, bail, ensure};
use repc::{Target, compute_layout, layout as l};
use std::collections::{HashMap, HashSet};
use windows_metadata::{reader::*, *};

#[cfg(test)]
mod foundation_tests;

fn builtin(b: l::BuiltinType) -> l::Type<()> {
    l::Type {
        layout: (),
        annotations: vec![],
        variant: l::TypeVariant::Builtin(b),
    }
}

// GUID's native ABI is four-byte aligned and sixteen bytes on all targets.
// Use a layout-only array, never CLR/reference-assembly implementation fields.
fn native_guid() -> l::Type<()> {
    l::Type {
        layout: (),
        annotations: vec![],
        variant: l::TypeVariant::Array(l::Array {
            element_type: Box::new(builtin(l::BuiltinType::UnsignedInt)),
            num_elements: Some(4),
        }),
    }
}

struct Layouts<'a> {
    db: &'a Db,
    arch: i32,
    target: Target,
    active: HashSet<EntityId>,
    cache: HashMap<EntityId, Computed>,
}
struct Computed {
    layout: l::TypeLayout,
    fields: Vec<FieldLayout>,
}
impl Layouts<'_> {
    fn sig(&mut self, ty: Type, owner: TypeDef<'_>) -> Result<(l::Type<()>, Vec<FieldLayout>)> {
        use l::BuiltinType as B;
        let ty = match ty {
            Type::Bool | Type::U8 => builtin(B::UnsignedChar),
            Type::I8 => builtin(B::SignedChar),
            Type::Char | Type::U16 => builtin(B::UnsignedShort),
            Type::I16 => builtin(B::Short),
            Type::U32 => builtin(B::UnsignedInt),
            Type::I32 => builtin(B::Int),
            Type::U64 => builtin(B::UnsignedLongLong),
            Type::I64 => builtin(B::LongLong),
            Type::F32 => builtin(B::Float),
            Type::F64 => builtin(B::Double),
            Type::ISize | Type::USize | Type::PtrMut(..) | Type::PtrConst(..) => {
                builtin(B::Pointer)
            }
            Type::ArrayFixed(e, n) => l::Type {
                layout: (),
                annotations: vec![],
                variant: l::TypeVariant::Array(l::Array {
                    element_type: Box::new(self.sig(*e, owner)?.0),
                    num_elements: Some(n as u64),
                }),
            },
            Type::ValueName(n)
                if self.db.index.foundation_type(&n) == Some(FoundationType::Guid) =>
            {
                native_guid()
            }
            Type::ValueName(n) | Type::ClassName(n) => {
                let computed = self.record(self.db.resolve(&n, owner, self.arch)?)?;
                // repc guarantees opaque substitution preserves containing layouts;
                // retain all alignment components, including MSVC required alignment.
                return Ok((
                    l::Type {
                        layout: (),
                        annotations: vec![],
                        variant: l::TypeVariant::Opaque(computed.layout),
                    },
                    computed.fields.clone(),
                ));
            }
            other => bail!("unlowered signature {other:?}"),
        };
        Ok((ty, vec![]))
    }
    fn record(&mut self, t: TypeDef<'_>) -> Result<&Computed> {
        ensure!(
            t.generic_params().next().is_none(),
            "open generic layout is not supported"
        );
        let key = crate::metadata::key(t, 2);
        if self.cache.contains_key(&key) {
            return Ok(&self.cache[&key]);
        }
        ensure!(self.active.insert(key), "by-value cycle");
        let result = (|| {
            let mut fields = vec![];
            if TypeDefOrRef::TypeDef(t).foundation_type() == Some(FoundationType::Guid) {
                return self.compute(native_guid(), fields);
            }
            match t.category() {
                TypeCategory::Delegate | TypeCategory::Interface => {
                    return self.compute(builtin(l::BuiltinType::Pointer), fields);
                }
                TypeCategory::Enum => {
                    let (ty, fields) = self.sig(
                        t.underlying_type()
                            .ok_or_else(|| anyhow::anyhow!("missing enum backing type"))?,
                        t,
                    )?;
                    return self.compute(ty, fields);
                }
                TypeCategory::Struct => {}
                _ => bail!("not a native value type"),
            }
            let explicit = t.flags().contains(TypeAttributes::ExplicitLayout);
            ensure!(
                explicit || t.flags().contains(TypeAttributes::SequentialLayout),
                "auto layout unsupported"
            );
            let mut annotations = vec![];
            if let Some(c) = t.class_layout() {
                ensure!(c.class_size() == 0, "declared size needs explicit adapter");
                if c.packing_size() != 0 {
                    annotations.push(l::Annotation::PragmaPack(c.packing_size() as u64 * 8));
                }
            }
            let mut alignments = native_alignment_attributes(t);
            if let Some(attribute) = alignments.next() {
                ensure!(
                    alignments.next().is_none(),
                    "duplicate native alignment attribute"
                );
                ensure!(
                    attribute.ctor().signature(&[]).types == [Type::I32],
                    "invalid native alignment constructor"
                );
                let Some(alignment) = native_alignment(&attribute) else {
                    bail!("invalid native alignment attribute");
                };
                ensure!(
                    alignment.is_power_of_two(),
                    "invalid native alignment attribute"
                );
                annotations.push(l::Annotation::Align(Some(alignment * 8)));
            }
            let mut inputs = vec![];
            for f in instance_fields(t, self.arch) {
                if explicit {
                    ensure!(
                        f.explicit_offset() == Some(0),
                        "nonzero/missing explicit offset needs dedicated layout adapter"
                    );
                } else {
                    ensure!(
                        f.explicit_offset().is_none(),
                        "unexpected offset on sequential type"
                    );
                }
                ensure!(
                    !f.has_attribute("FlexibleArrayAttribute"),
                    "flexible array policy not implemented"
                );
                let (ty, nested) = self.sig(f.ty(), t)?;
                fields.push(FieldLayout {
                    name: f.name().into(),
                    id: crate::metadata::key(f, 4),
                    offset_bits: 0,
                    size_bits: 0,
                    bitfields: crate::metadata::bitfields(f)?,
                    fields: nested,
                });
                inputs.push(l::RecordField {
                    layout: Some(()),
                    annotations: vec![],
                    named: true,
                    bit_width: None,
                    ty,
                });
            }
            self.compute(
                l::Type {
                    layout: (),
                    annotations,
                    variant: l::TypeVariant::Record(l::Record {
                        kind: if explicit {
                            l::RecordKind::Union
                        } else {
                            l::RecordKind::Struct
                        },
                        fields: inputs,
                    }),
                },
                fields,
            )
        })();
        self.active.remove(&key);
        self.cache.insert(key, result?);
        Ok(&self.cache[&key])
    }
    fn compute(&self, input: l::Type<()>, mut fields: Vec<FieldLayout>) -> Result<Computed> {
        let output = compute_layout(self.target, &input).map_err(|e| anyhow::anyhow!("{e:?}"))?;
        if let l::TypeVariant::Record(record) = &output.variant {
            for (field, computed) in fields.iter_mut().zip(&record.fields) {
                let position = computed
                    .layout
                    .ok_or_else(|| anyhow::anyhow!("missing field layout"))?;
                for bitfield in &field.bitfields {
                    ensure!(
                        bitfield
                            .relative_offset_bits
                            .checked_add(bitfield.width_bits)
                            .is_some_and(|end| end <= position.size_bits),
                        "bitfield exceeds backing field"
                    );
                }
                field.offset_bits = position.offset_bits;
                field.size_bits = position.size_bits;
            }
        }
        Ok(Computed {
            layout: output.layout,
            fields,
        })
    }
}

pub(crate) fn derive_key(db: &Db, key: EntityId, arch: Architecture) -> Result<Layout> {
    let bit = arch.bit();
    let t: TypeDef = db.row(key, 2)?;
    ensure!(supports(t, bit), "type unavailable on architecture");
    let name = t.name();
    let mut layouts = Layouts {
        db,
        arch: bit,
        target: match arch {
            Architecture::X86 => Target::I686PcWindowsMsvc,
            Architecture::X64 => Target::X86_64PcWindowsMsvc,
            Architecture::Arm64 => Target::Aarch64PcWindowsMsvc,
        },
        active: HashSet::new(),
        cache: HashMap::new(),
    };
    layouts.record(t)?;
    let dependencies_loaded = layouts.cache.len();
    let output = layouts.cache.remove(&key).expect("computed root layout");
    Ok(Layout {
        name: name.into(),
        architecture: arch,
        id: key,
        size_bits: output.layout.size_bits,
        alignment_bits: output.layout.pointer_alignment_bits,
        fields: output.fields,
        dependencies_loaded,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{FixtureFile, catalog, type_id};

    #[test]
    fn containing_pack_preserves_repeated_record_layout_and_array_stride() {
        for aligned in [false, true] {
            for pack in [1, 2, 8] {
                let mut file = writer::File::new("NestedPacking");
                let base = file.TypeRef("System", "ValueType");
                let leaf = file.TypeDef(
                    "Probe",
                    "Leaf",
                    writer::TypeDefOrRef::TypeRef(base),
                    TypeAttributes::Public | TypeAttributes::SequentialLayout,
                );
                file.Field("tag", &Type::U8, FieldAttributes::Public);
                file.Field("value", &Type::U32, FieldAttributes::Public);
                if aligned {
                    let attribute = file.TypeRef("Winlex.Metadata", "NativeAlignmentAttribute");
                    let ctor = file.constructor(attribute, vec![Type::I32]);
                    file.Attribute(
                        writer::HasAttribute::TypeDef(leaf),
                        writer::AttributeType::MemberRef(ctor),
                        &[(String::new(), Value::I32(16))],
                    );
                }
                let parent = file.TypeDef(
                    "Probe",
                    "Parent",
                    writer::TypeDefOrRef::TypeRef(base),
                    TypeAttributes::Public | TypeAttributes::SequentialLayout,
                );
                file.ClassLayout(parent, pack, 0);
                for (name, ty) in [
                    ("header", Type::U8),
                    ("first", Type::value_named("Probe", "Leaf")),
                    ("second", Type::value_named("Probe", "Leaf")),
                    (
                        "array",
                        Type::ArrayFixed(Box::new(Type::value_named("Probe", "Leaf")), 2),
                    ),
                    ("tail", Type::U8),
                ] {
                    file.Field(name, &ty, FieldAttributes::Public);
                }
                let db = catalog("NestedPacking", file);
                // MSVC ABI: parent packing caps natural alignment, not a nested
                // __declspec(align(16)); the leaf's own member offsets never change.
                let (size, alignment, offsets, stride) = if aligned {
                    (96, 16, [0, 16, 32, 48, 80], 16)
                } else {
                    match pack {
                        1 => (34, 1, [0, 1, 9, 17, 33], 8),
                        2 => (36, 2, [0, 2, 10, 18, 34], 8),
                        _ => (40, 4, [0, 4, 12, 20, 36], 8),
                    }
                };
                for arch in Architecture::ALL {
                    let layout = db.layout(type_id(&db, "Parent"), arch).unwrap();
                    assert_eq!(
                        (layout.size_bits, layout.alignment_bits),
                        (size * 8, alignment * 8)
                    );
                    assert_eq!(
                        layout
                            .fields
                            .iter()
                            .map(|f| f.offset_bits)
                            .collect::<Vec<_>>(),
                        offsets.map(|v| v * 8)
                    );
                    assert_eq!(layout.fields[3].size_bits, stride * 2 * 8);
                    for field in &layout.fields[1..3] {
                        assert_eq!(field.fields[1].offset_bits, 32);
                        assert_eq!(field.fields[1].size_bits, 32);
                    }
                }
            }
        }
    }

    #[test]
    fn recursive_values_are_rejected_but_recursive_pointers_have_native_layout() {
        for pointer in [false, true] {
            let mut file = writer::File::new("RecursiveLayout");
            let base = file.TypeRef("System", "ValueType");
            for (name, other) in [("First", "Second"), ("Second", "First")] {
                file.TypeDef(
                    "Probe",
                    name,
                    writer::TypeDefOrRef::TypeRef(base),
                    TypeAttributes::Public | TypeAttributes::SequentialLayout,
                );
                let ty = Type::value_named("Probe", other);
                file.Field(
                    "next",
                    &if pointer {
                        Type::PtrMut(Box::new(ty), 1)
                    } else {
                        ty
                    },
                    FieldAttributes::Public,
                );
            }
            let db = catalog("RecursiveLayout", file);
            for arch in Architecture::ALL {
                let result = db.layout(type_id(&db, "First"), arch);
                if pointer {
                    let layout = result.unwrap();
                    assert_eq!(
                        (layout.size_bits, layout.alignment_bits),
                        (
                            arch.pointer_bytes() as u64 * 8,
                            arch.pointer_bytes() as u64 * 8
                        )
                    );
                } else {
                    assert!(
                        result.is_err(),
                        "by-value recursion cannot have finite size"
                    );
                }
            }
        }
    }

    #[test]
    fn explicit_alignment_propagates_through_unions_and_containing_records() {
        let mut file = writer::File::new("AlignedRecords");
        let base = file.TypeRef("System", "ValueType");
        let attribute = file.TypeRef("Winlex.Metadata", "NativeAlignmentAttribute");
        let constructor = file.constructor(attribute, vec![Type::I32]);
        for (name, union, fields) in [
            (
                "Aligned",
                false,
                vec![("data", Type::ArrayFixed(Box::new(Type::U64), 2))],
            ),
            (
                "Overlay",
                true,
                vec![
                    ("value", Type::value_named("Probe", "Aligned")),
                    ("pointer", Type::PtrMut(Box::new(Type::Void), 1)),
                ],
            ),
            (
                "Value",
                false,
                vec![
                    ("value", Type::value_named("Probe", "Overlay")),
                    ("flags", Type::U32),
                ],
            ),
            (
                "Record",
                false,
                vec![
                    ("header", Type::ArrayFixed(Box::new(Type::U32), 4)),
                    ("first", Type::value_named("Probe", "Value")),
                    ("second", Type::value_named("Probe", "Value")),
                    ("third", Type::value_named("Probe", "Value")),
                ],
            ),
        ] {
            let record = file.TypeDef(
                "Probe",
                name,
                writer::TypeDefOrRef::TypeRef(base),
                TypeAttributes::Public
                    | TypeAttributes::Sealed
                    | if union {
                        TypeAttributes::ExplicitLayout
                    } else {
                        TypeAttributes::SequentialLayout
                    },
            );
            if name == "Aligned" {
                file.Attribute(
                    writer::HasAttribute::TypeDef(record),
                    writer::AttributeType::MemberRef(constructor),
                    &[(String::new(), Value::I32(16))],
                );
            }
            for (name, ty) in fields {
                let field = file.Field(name, &ty, FieldAttributes::Public);
                if union {
                    file.FieldLayout(field, 0);
                }
            }
        }
        let db = catalog("AlignedRecords", file);
        for arch in Architecture::ALL {
            for (name, size) in [
                ("Aligned", 16),
                ("Overlay", 16),
                ("Value", 32),
                ("Record", 112),
            ] {
                let layout = db.layout(type_id(&db, name), arch).unwrap();
                assert_eq!(layout.size_bits, size * 8, "{arch:?}/{name}");
                assert_eq!(layout.alignment_bits, 128, "{arch:?}/{name}");
                if name == "Record" {
                    assert_eq!(
                        layout
                            .fields
                            .iter()
                            .map(|field| field.offset_bits)
                            .collect::<Vec<_>>(),
                        [0, 128, 384, 640]
                    );
                }
            }
        }
    }

    #[test]
    fn invalid_explicit_alignment_is_rejected() {
        for (alignment, argument_name, copies) in [
            (Value::I32(-1), "", 1),
            (Value::I32(0), "", 1),
            (Value::I32(3), "", 1),
            (Value::U32(16), "", 1),
            (Value::Utf8("16".into()), "", 1),
            (Value::I32(16), "Alignment", 1),
            (Value::I32(16), "", 2),
        ] {
            let mut file = writer::File::new("InvalidAlignment");
            let base = file.TypeRef("System", "ValueType");
            let attribute = file.TypeRef("Winlex.Metadata", "NativeAlignmentAttribute");
            let constructor = file.constructor(
                attribute,
                if argument_name.is_empty() {
                    vec![alignment.ty()]
                } else {
                    vec![]
                },
            );
            let record = file.TypeDef(
                "Probe",
                "Record",
                writer::TypeDefOrRef::TypeRef(base),
                TypeAttributes::Public | TypeAttributes::SequentialLayout,
            );
            file.Field("value", &Type::U32, FieldAttributes::Public);
            for _ in 0..copies {
                file.Attribute(
                    writer::HasAttribute::TypeDef(record),
                    writer::AttributeType::MemberRef(constructor),
                    &[(argument_name.into(), alignment.clone())],
                );
            }
            let db = catalog("InvalidAlignment", file);
            for arch in Architecture::ALL {
                assert!(db.layout(type_id(&db, "Record"), arch).is_err());
            }
        }
    }

    #[test]
    fn native_alignment_is_distinct_from_pack_and_requires_its_full_name() {
        for (namespace, name, pack, alignment, expected_size, expected_alignment, offset) in [
            (
                "Winlex.Metadata",
                "NativeAlignmentAttribute",
                0,
                16,
                16,
                16,
                4,
            ),
            (
                "Winlex.Metadata",
                "NativeAlignmentAttribute",
                1,
                16,
                16,
                16,
                1,
            ),
            (
                "Winlex.Metadata",
                "NativeAlignmentAttribute",
                16,
                1,
                8,
                4,
                4,
            ),
            ("Other", "NativeAlignmentAttribute", 0, 16, 8, 4, 4),
            (
                "Windows.Win32.Metadata",
                "AlignmentAttribute",
                0,
                16,
                8,
                4,
                4,
            ),
        ] {
            let mut file = writer::File::new("Packing");
            let base = file.TypeRef("System", "ValueType");
            let attribute = file.TypeRef(namespace, name);
            let constructor = file.constructor(attribute, vec![Type::I32]);
            let record = file.TypeDef(
                "Probe",
                "Record",
                writer::TypeDefOrRef::TypeRef(base),
                TypeAttributes::Public | TypeAttributes::SequentialLayout,
            );
            file.ClassLayout(record, pack, 0);
            file.Field("header", &Type::U8, FieldAttributes::Public);
            file.Field("value", &Type::U32, FieldAttributes::Public);
            file.Attribute(
                writer::HasAttribute::TypeDef(record),
                writer::AttributeType::MemberRef(constructor),
                &[(String::new(), Value::I32(alignment))],
            );
            let db = catalog("Packing", file);
            for arch in Architecture::ALL {
                let layout = db.layout(type_id(&db, "Record"), arch).unwrap();
                assert_eq!(
                    (layout.size_bits, layout.alignment_bits),
                    (expected_size * 8, expected_alignment * 8)
                );
                assert_eq!(layout.fields[1].offset_bits, offset * 8);
            }
        }
    }

    #[test]
    fn architecture_merge_preserves_native_alignment_variants() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tmp")
            .join(format!(
                "alignment-merge-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
        std::fs::create_dir_all(&dir).unwrap();
        let mut merger = windows_metadata::merge::Merger::new();
        let mut inputs = vec![];
        for (arch, alignment) in [(1, 8), (2, 16), (4, 16)] {
            let mut file = writer::File::new("ArchAlignment");
            let base = file.TypeRef("System", "ValueType");
            let attribute = file.TypeRef("Winlex.Metadata", "NativeAlignmentAttribute");
            let constructor = file.constructor(attribute, vec![Type::I32]);
            let record = file.TypeDef(
                "Probe",
                "Record",
                writer::TypeDefOrRef::TypeRef(base),
                TypeAttributes::Public | TypeAttributes::SequentialLayout,
            );
            file.Field("value", &Type::U32, FieldAttributes::Public);
            file.Attribute(
                writer::HasAttribute::TypeDef(record),
                writer::AttributeType::MemberRef(constructor),
                &[(String::new(), Value::I32(alignment))],
            );
            let ns = "Windows.Win32.Foundation.Metadata";
            let attribute = file.TypeRef(ns, "SupportedArchitectureAttribute");
            let constructor =
                file.constructor(attribute, vec![Type::value_named(ns, "Architecture")]);
            file.Attribute(
                writer::HasAttribute::TypeDef(record),
                writer::AttributeType::MemberRef(constructor),
                &[(
                    String::new(),
                    Value::EnumValue(TypeName::named(ns, "Architecture"), Box::new(Value::I32(7))),
                )],
            );
            let path = dir.join(format!("{arch}.winmd"));
            std::fs::write(&path, file.into_stream()).unwrap();
            merger.arch_input(&path, arch);
            inputs.push(path);
        }
        let output = dir.join("Merged.winmd");
        merger.output(&output).merge().unwrap();
        let union = dir.join("Union.winmd");
        windows_metadata::merge::Merger::new()
            .input(&output)
            .union_enums()
            .output(&union)
            .merge()
            .unwrap();
        for path in [&output, &union] {
            let db = crate::Catalog::from_inputs([crate::SourceData {
                path: path.clone(),
                bytes: std::fs::read(path).unwrap().into(),
            }])
            .unwrap();
            for arch in Architecture::ALL {
                let row = crate::test_support::first_match(db.search_exact(
                    "Record",
                    crate::SearchOptions {
                        architecture: Some(arch),
                        ..Default::default()
                    },
                ));
                let layout = db.layout(row.id, arch).unwrap();
                let expected = if arch == Architecture::X86 { 64 } else { 128 };
                assert_eq!(
                    (layout.size_bits, layout.alignment_bits),
                    (expected, expected)
                );
            }
        }
        for path in inputs.into_iter().chain([output, union]) {
            std::fs::remove_file(path).unwrap();
        }
        std::fs::remove_dir(dir).unwrap();
    }
}
