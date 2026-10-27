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
            let mut fields = vec![];
            let arch = self.arch;
            for f in t
                .fields()
                .filter(|f| !f.flags().contains(FieldAttributes::Static) && supports(*f, arch))
            {
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
            for (f, computed) in t
                .fields()
                .filter(|f| !f.flags().contains(FieldAttributes::Static) && supports(*f, bit))
                .zip(&record.fields)
            {
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
