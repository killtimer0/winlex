use crate::metadata::{Db, supports};
use crate::source::EntityId;
use crate::{Architecture, Layout};
use anyhow::{Result, bail, ensure};
use repc::{Target, compute_layout, layout as l};
use std::collections::{HashMap, HashSet};
use windows_metadata::{reader::*, *};

fn builtin(b: l::BuiltinType) -> l::Type<()> {
    l::Type {
        layout: (),
        annotations: vec![],
        variant: l::TypeVariant::Builtin(b),
    }
}

struct Lower<'a> {
    db: &'a Db,
    arch: i32,
    active: HashSet<EntityId>,
    cache: HashMap<EntityId, l::Type<()>>,
}
fn instance_fields(t: TypeDef<'_>, arch: i32) -> impl Iterator<Item = Field<'_>> {
    t.fields()
        .filter(move |f| !f.flags().contains(FieldAttributes::Static) && supports(*f, arch))
}
impl Lower<'_> {
    fn sig(&mut self, ty: Type, owner: TypeDef<'_>) -> Result<l::Type<()>> {
        use l::BuiltinType as B;
        Ok(match ty {
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
                    element_type: Box::new(self.sig(*e, owner)?),
                    num_elements: Some(n as u64),
                }),
            },
            Type::ValueName(n) | Type::ClassName(n) => {
                self.record(self.db.resolve(&n, owner, self.arch)?)?
            }
            other => bail!("unlowered signature {other:?}"),
        })
    }
    fn record(&mut self, t: TypeDef<'_>) -> Result<l::Type<()>> {
        ensure!(
            t.generic_params().next().is_none(),
            "open generic layout is not supported"
        );
        let key = crate::metadata::key(t, 2);
        if let Some(x) = self.cache.get(&key) {
            return Ok(x.clone());
        }
        ensure!(self.active.insert(key), "by-value cycle");
        let result = (|| {
            match t.category() {
                TypeCategory::Delegate | TypeCategory::Interface => {
                    return Ok(builtin(l::BuiltinType::Pointer));
                }
                TypeCategory::Enum => {
                    return self.sig(
                        t.underlying_type()
                            .ok_or_else(|| anyhow::anyhow!("missing enum backing type"))?,
                        t,
                    );
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
            let mut alignments = t.attributes().filter(|attribute| {
                attribute.namespace() == "Winlex.Metadata"
                    && attribute.name() == "NativeAlignmentAttribute"
            });
            if let Some(attribute) = alignments.next() {
                ensure!(
                    alignments.next().is_none(),
                    "duplicate native alignment attribute"
                );
                ensure!(
                    attribute.ctor().signature(&[]).types == [Type::I32],
                    "invalid native alignment constructor"
                );
                let values = attribute.value();
                let alignment = match values.as_slice() {
                    [(name, Value::I32(value))] if name.is_empty() && *value > 0 => *value as u32,
                    _ => bail!("invalid native alignment attribute"),
                };
                ensure!(
                    alignment.is_power_of_two(),
                    "invalid native alignment attribute"
                );
                annotations.push(l::Annotation::Align(Some(u64::from(alignment) * 8)));
            }
            let mut fields = vec![];
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
                fields.push(l::RecordField {
                    layout: Some(()),
                    annotations: vec![],
                    named: true,
                    bit_width: None,
                    ty: self.sig(f.ty(), t)?,
                });
            }
            Ok(l::Type {
                layout: (),
                annotations,
                variant: l::TypeVariant::Record(l::Record {
                    kind: if explicit {
                        l::RecordKind::Union
                    } else {
                        l::RecordKind::Struct
                    },
                    fields,
                }),
            })
        })();
        self.active.remove(&key);
        if let Ok(v) = &result {
            self.cache.insert(key, v.clone());
        }
        result
    }
}

pub(crate) fn derive_key(db: &Db, key: EntityId, arch: Architecture) -> Result<Layout> {
    let bit = arch.bit();
    let t: TypeDef = db.row(key, 2)?;
    ensure!(supports(t, bit), "type unavailable on architecture");
    let name = t.name();
    let mut lower = Lower {
        db,
        arch: bit,
        active: HashSet::new(),
        cache: HashMap::new(),
    };
    let input = lower.record(t)?;
    let target = match arch {
        Architecture::X86 => Target::I686PcWindowsMsvc,
        Architecture::X64 => Target::X86_64PcWindowsMsvc,
        Architecture::Arm64 => Target::Aarch64PcWindowsMsvc,
    };
    let output = compute_layout(target, &input).map_err(|e| anyhow::anyhow!("{e:?}"))?;
    fn render(
        db: &Db,
        t: TypeDef<'_>,
        output: &l::Type<l::TypeLayout>,
        bit: i32,
    ) -> Result<Vec<crate::FieldLayout>> {
        let mut fields = vec![];
        if let l::TypeVariant::Record(record) = &output.variant {
            for (f, computed) in instance_fields(t, bit).zip(&record.fields) {
                let position = computed
                    .layout
                    .ok_or_else(|| anyhow::anyhow!("missing field layout"))?;
                let bits = crate::metadata::bitfields(f)?;
                for b in &bits {
                    let offset = b.relative_offset_bits;
                    let width = b.width_bits;
                    ensure!(
                        offset
                            .checked_add(width)
                            .is_some_and(|end| end <= position.size_bits),
                        "bitfield exceeds backing field"
                    );
                }
                let nested = if let l::TypeVariant::Record(_) = &computed.ty.variant
                    && let Type::ValueName(n) | Type::ClassName(n) = f.ty()
                {
                    render(db, db.resolve(&n, t, bit)?, &computed.ty, bit)?
                } else {
                    vec![]
                };
                fields.push(crate::FieldLayout {
                    name: f.name().into(),
                    id: crate::metadata::key(f, 4),
                    offset_bits: position.offset_bits,
                    size_bits: position.size_bits,
                    bitfields: bits,
                    fields: nested,
                });
            }
        }
        Ok(fields)
    }
    let fields = render(db, t, &output, bit)?;
    Ok(Layout {
        name: name.into(),
        architecture: arch,
        id: key,
        size_bits: output.layout.size_bits,
        alignment_bits: output.layout.pointer_alignment_bits,
        fields,
        dependencies_loaded: lower.cache.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{FixtureFile, catalog, type_id};

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
                let row = db
                    .search_exact(
                        "Record",
                        crate::SearchOptions {
                            architecture: Some(arch),
                            ..Default::default()
                        },
                    )
                    .unwrap()
                    .next()
                    .unwrap()
                    .unwrap();
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
