use crate::metadata::{Db, arch_bit, supports, token};
use anyhow::{Result, bail, ensure};
use repc::{Target, compute_layout, layout as l};
use serde_json::{Value, json};
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
    active: HashSet<usize>,
    cache: HashMap<usize, l::Type<()>>,
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
        if let Some(x) = self.cache.get(&t.pos()) {
            return Ok(x.clone());
        }
        ensure!(self.active.insert(t.pos()), "by-value cycle");
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
        self.active.remove(&t.pos());
        if let Ok(v) = &result {
            self.cache.insert(t.pos(), v.clone());
        }
        result
    }
}

pub fn derive(db: &Db, name: &str, arch: &str) -> Result<Value> {
    let bit = arch_bit(arch)?;
    let t = db.find(name, bit)?;
    derive_token(db, token(t, 2), arch)
}

pub fn derive_token(db: &Db, tok: u32, arch: &str) -> Result<Value> {
    let bit = arch_bit(arch)?;
    ensure!(
        tok >> 24 == 2 && tok & 0xffffff != 0 && (tok & 0xffffff) as usize <= db.type_count,
        "invalid type token"
    );
    let t = db.ty((tok & 0xffffff) as usize - 1);
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
        "x86" => Target::I686PcWindowsMsvc,
        "x64" => Target::X86_64PcWindowsMsvc,
        "arm64" => Target::Aarch64PcWindowsMsvc,
        _ => unreachable!(),
    };
    let output = compute_layout(target, &input).map_err(|e| anyhow::anyhow!("{e:?}"))?;
    fn render(
        db: &Db,
        t: TypeDef<'_>,
        output: &l::Type<l::TypeLayout>,
        bit: i32,
    ) -> Result<Vec<Value>> {
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
                let mut field = json!({"name":f.name(),"token":format!("0x{:08x}",token(f,4)),"offset_bits":position.offset_bits,"size_bits":position.size_bits});
                let mut bits = crate::metadata::bitfields(f)?;
                for b in &mut bits {
                    let offset = b["relative_offset_bits"].as_u64().unwrap();
                    let width = b["width_bits"].as_u64().unwrap();
                    ensure!(
                        offset
                            .checked_add(width)
                            .is_some_and(|end| end <= position.size_bits),
                        "bitfield exceeds backing field"
                    );
                    b["offset_bits"] = json!(position.offset_bits + offset);
                }
                field["bitfields"] = json!(bits);
                if let l::TypeVariant::Record(_) = &computed.ty.variant
                    && let Type::ValueName(n) | Type::ClassName(n) = f.ty()
                {
                    field["fields"] =
                        json!(render(db, db.resolve(&n, t, bit)?, &computed.ty, bit)?);
                }
                fields.push(field);
            }
        }
        Ok(fields)
    }
    let fields = render(db, t, &output, bit)?;
    Ok(
        json!({"name":name,"arch":arch,"token":format!("0x{:08x}",token(t,2)),"size_bits":output.layout.size_bits,"alignment_bits":output.layout.pointer_alignment_bits,"fields":fields,"dependencies_loaded":lower.cache.len(),"backend":"repc 0.1.1","scope":"sequential and all-zero explicit overlays"}),
    )
}
