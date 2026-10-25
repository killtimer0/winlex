use anyhow::{Result, bail, ensure};
use serde_json::{Value as J, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashSet},
    path::Path,
};
use windows_metadata::{reader::*, *};

pub struct Db {
    pub index: Index,
    pub sha: String,
    pub type_count: usize,
    pub field_count: usize,
    pub method_count: usize,
}

pub fn token<'a, R: AsRow<'a>>(r: R, table: u32) -> u32 {
    table << 24 | (r.pos() + 1) as u32
}
pub fn attrs<'a, R: HasAttributes<'a>>(r: R) -> J {
    json!(
        r.attributes()
            .map(|a| json!({"name":a.ctor().parent().name(),"value":format!("{:?}", a.value())}))
            .collect::<Vec<_>>()
    )
}
pub fn arch_bit(arch: &str) -> Result<i32> {
    Ok(match arch {
        "x86" => 1,
        "x64" => 2,
        "arm64" => 4,
        _ => bail!("unknown arch"),
    })
}
pub fn supports<'a>(r: impl HasAttributes<'a>, arch: i32) -> bool {
    r.arches() == 0 || r.arches() & arch != 0
}

pub fn bitfields(f: Field<'_>) -> Result<Vec<J>> {
    let mut result = vec![];
    for a in f
        .attributes()
        .filter(|a| a.name() == "NativeBitfieldAttribute")
    {
        let values = a.value();
        ensure!(values.len() == 3, "unsupported NativeBitfield encoding");
        let name = match &values[0].1 {
            Value::Utf8(s) | Value::Utf16(s) => s,
            _ => bail!("invalid bitfield name"),
        };
        let num = |v: &Value| -> Result<u64> {
            Ok(match v {
                Value::I64(v) => u64::try_from(*v)?,
                Value::U64(v) => *v,
                Value::I32(v) => u64::try_from(*v)?,
                Value::U32(v) => *v as u64,
                _ => bail!("invalid bitfield integer"),
            })
        };
        result.push(json!({"name":name,"relative_offset_bits":num(&values[1].1)?,"width_bits":num(&values[2].1)?}));
    }
    Ok(result)
}

impl Db {
    pub fn reader_facts(&self) -> J {
        fn refs(t: &Type, out: &mut Vec<u32>) {
            match t {
                Type::ValueName(n) | Type::ClassName(n) => {
                    if let Some((_, token)) = n.identity {
                        out.push(token);
                    }
                    for g in &n.generics {
                        refs(g, out);
                    }
                }
                Type::Array(t)
                | Type::ArrayFixed(t, _)
                | Type::PtrMut(t, _)
                | Type::PtrConst(t, _)
                | Type::RefMut(t)
                | Type::RefConst(t) => refs(t, out),
                _ => {}
            }
        }
        let layouts: Vec<_> = self
            .index
            .rows::<FieldLayout>(0)
            .map(|r| json!({"field":token(r.field(),4),"offset":r.offset()}))
            .collect();
        let scopes: Vec<_> = self.index.rows::<TypeRef>(0).map(|r|json!({"token":token(r,1),"scope":r.scope_token(),"name":r.name(),"namespace":r.namespace()})).collect();
        let mut signatures = BTreeMap::new();
        for f in self.index.rows::<Field>(0) {
            let mut r = vec![];
            refs(&f.ty(), &mut r);
            signatures.insert(token(f, 4).to_string(), r);
        }
        for m in self.index.rows::<MethodDef>(0) {
            let s = m.signature(&[]);
            let mut r = vec![];
            refs(&s.return_type, &mut r);
            for t in &s.types {
                refs(t, &mut r);
            }
            signatures.insert(token(m, 6).to_string(), r);
        }
        json!({"layouts":layouts,"scopes":scopes,"signature_type_tokens":signatures})
    }
    pub fn open(path: &Path) -> Result<Self> {
        let bytes = std::fs::read(path)?;
        let sha = format!("{:x}", Sha256::digest(&bytes));
        let file = File::new(bytes).ok_or_else(|| anyhow::anyhow!("invalid metadata"))?;
        let index = Index::new(vec![file]);
        let type_count = index.rows::<TypeDef>(0).len();
        let field_count = index.rows::<Field>(0).len();
        let method_count = index.rows::<MethodDef>(0).len();
        Ok(Self {
            index,
            sha,
            type_count,
            field_count,
            method_count,
        })
    }
    pub fn ty(&self, pos: usize) -> TypeDef<'_> {
        TypeDef::from_row(Row {
            index: &self.index,
            file: 0,
            pos,
        })
    }
    pub fn field(&self, pos: usize) -> Field<'_> {
        Field::from_row(Row {
            index: &self.index,
            file: 0,
            pos,
        })
    }
    pub fn method(&self, pos: usize) -> MethodDef<'_> {
        MethodDef::from_row(Row {
            index: &self.index,
            file: 0,
            pos,
        })
    }
    pub fn find(&self, name: &str, arch: i32) -> Result<TypeDef<'_>> {
        let found: Vec<_> = (0..self.type_count)
            .map(|p| self.ty(p))
            .filter(|t| {
                (t.name() == name || format!("{}.{}", t.namespace(), t.name()) == name)
                    && supports(*t, arch)
            })
            .collect();
        ensure!(
            found.len() == 1,
            "expected one type {name}, found {}",
            found.len()
        );
        Ok(found[0])
    }
    pub fn resolve(&self, n: &TypeName, owner: TypeDef<'_>, arch: i32) -> Result<TypeDef<'_>> {
        ensure!(n.generics.is_empty(), "generic type unsupported");
        if let Some((file, tok)) = n.identity {
            ensure!(file == 0, "external file unsupported");
            if tok >> 24 == 2 {
                let t = self.ty((tok & 0xffffff) as usize - 1);
                ensure!(
                    supports(t, arch),
                    "TypeDef reference unavailable on architecture"
                );
                return Ok(t);
            }
            ensure!(tok >> 24 == 1, "unhandled signature reference token");
            let r = TypeRef::from_row(Row {
                index: &self.index,
                file,
                pos: (tok & 0xffffff) as usize - 1,
            });
            let scope = r.scope_token();
            if scope >> 24 == 1 {
                let parent = TypeRef::from_row(Row {
                    index: &self.index,
                    file,
                    pos: (scope & 0xffffff) as usize - 1,
                });
                let mut pn = TypeName::named(parent.namespace(), parent.name());
                pn.identity = Some((file, scope));
                let p = self.resolve(&pn, owner, arch)?;
                let found: Vec<_> = self
                    .index
                    .nested(p)
                    .filter(|t| t.name() == n.name && supports(*t, arch))
                    .collect();
                ensure!(found.len() == 1, "ambiguous nested reference");
                return Ok(found[0]);
            }
            ensure!(
                scope == 1 || scope == 0,
                "external ResolutionScope 0x{scope:08x} unsupported for {n:?}"
            );
        }
        let found: Vec<_> = if n.namespace.is_empty() {
            self.index
                .nested(owner)
                .filter(|t| t.name() == n.name && supports(*t, arch))
                .collect()
        } else {
            self.index
                .get(&n.namespace, &n.name)
                .filter(|t| supports(*t, arch))
                .collect()
        };
        ensure!(found.len() == 1, "unresolved/ambiguous type {n:?}");
        Ok(found[0])
    }
    pub fn show(&self, tok: u32) -> Result<J> {
        let p = (tok & 0xffffff)
            .checked_sub(1)
            .ok_or_else(|| anyhow::anyhow!("nil token"))? as usize;
        Ok(match tok >> 24 {
            2 if p < self.type_count => {
                let t = self.ty(p);
                json!({"token":format!("0x{tok:08x}"),"name":t.name(),"namespace":t.namespace(),"flags":format!("{:?}",t.flags()),"attributes":attrs(t),
                    "layout":t.class_layout().map(|l|json!({"pack":l.packing_size(),"declared_size":l.class_size()})),
                    "interfaces":t.interface_impls().map(|i|format!("{:?}",i.interface(&[]))).collect::<Vec<_>>(),
                    "fields":t.fields().map(|f|self.show(token(f,4))).collect::<Result<Vec<_>>>()?,
                    "methods":t.methods().map(|m|self.show(token(m,6))).collect::<Result<Vec<_>>>()?})
            }
            4 if p < self.field_count => {
                let f = self.field(p);
                json!({"token":format!("0x{tok:08x}"),"name":f.name(),"signature":format!("{:?}",f.ty()),"declared_offset":f.explicit_offset(),"attributes":attrs(f),"constant":f.constant().map(|c|format!("{:?}",c.value()))})
            }
            6 if p < self.method_count => {
                let m = self.method(p);
                json!({"token":format!("0x{tok:08x}"),"name":m.name(),"signature":format!("{:?}",m.signature(&[])),"attributes":attrs(m),"params":m.params().map(|p|json!({"name":p.name(),"sequence":p.sequence(),"attributes":attrs(p)})).collect::<Vec<_>>()})
            }
            _ => bail!("unsupported or out-of-range token 0x{tok:08x}"),
        })
    }
    pub fn audit(&self) -> J {
        let mut counts = BTreeMap::<String, usize>::new();
        let mut failures = Vec::new();
        // This reader uses assertions/panics for unsupported encodings. Catch them for
        // the audit only; never interpret a failed decode as an absent declaration.
        let old = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        for p in 0..self.type_count {
            let t = self.ty(p);
            let category = if t.name() == "<Module>" {
                "Module".into()
            } else {
                format!("{:?}", t.category())
            };
            *counts.entry(category).or_default() += 1;
            if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| attrs(t))).is_err() {
                failures.push(json!({"kind":"attributes","token":token(t,2)}));
            }
        }
        let mut constants = 0;
        let mut attributes = 0;
        for p in 0..self.field_count {
            let f = self.field(p);
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                f.ty();
                attrs(f);
                if let Some(c) = f.constant() {
                    c.value();
                    true
                } else {
                    false
                }
            }));
            match r {
                Ok(true) => constants += 1,
                Ok(false) => {}
                Err(_) => failures.push(json!({"kind":"field","token":token(f,4)})),
            }
            attributes += f.attributes().count();
        }
        for p in 0..self.method_count {
            let m = self.method(p);
            if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let s = m.signature(&[]);
                m.params_by_sequence(s.types.len()).unwrap();
                attrs(m);
                for p in m.params() {
                    attrs(p);
                }
            }))
            .is_err()
            {
                failures.push(json!({"kind":"method","token":token(m,6)}));
            }
        }
        std::panic::set_hook(old);
        json!({"sha256":self.sha,"type_rows":self.type_count,"field_rows":self.field_count,"method_rows":self.method_count,"categories":counts,"constant_rows_decoded":constants,"field_attributes":attributes,"failures":failures,
            "reader_limits":["TypeSpec expansion and modifiers are not lossless signature ASTs","Type enum has no general function-pointer variant"]})
    }
    pub fn vtable(&self, name: &str, arch: &str) -> Result<J> {
        let bit = arch_bit(arch)?;
        let t = self.find(name, bit)?;
        self.vtable_token(token(t, 2), arch)
    }
    pub fn vtable_token(&self, tok: u32, arch: &str) -> Result<J> {
        let bit = arch_bit(arch)?;
        ensure!(
            tok >> 24 == 2 && tok & 0xffffff != 0 && (tok & 0xffffff) as usize <= self.type_count,
            "invalid type token"
        );
        let t = self.ty((tok & 0xffffff) as usize - 1);
        ensure!(supports(t, bit), "interface unavailable on architecture");
        let name = t.name();
        let mut slots = Vec::new();
        let mut active = HashSet::new();
        fn visit(
            db: &Db,
            t: TypeDef<'_>,
            bit: i32,
            active: &mut HashSet<usize>,
            slots: &mut Vec<J>,
        ) -> Result<()> {
            ensure!(t.category() == TypeCategory::Interface, "not an interface");
            ensure!(active.insert(t.pos()), "interface cycle");
            let bases: Vec<_> = t.interface_impls().collect();
            ensure!(bases.len() <= 1, "multiple interface bases unsupported");
            for b in bases {
                let n = match b.interface(&[]) {
                    Type::ClassName(n) | Type::ValueName(n) => n,
                    _ => bail!("unsupported interface reference"),
                };
                visit(db, db.resolve(&n, t, bit)?, bit, active, slots)?;
            }
            for m in t.methods().filter(|m| supports(*m, bit)) {
                ensure!(
                    !m.flags().contains(MethodAttributes::Static),
                    "static interface method unsupported"
                );
                slots.push(json!({"index":slots.len(),"method":m.name(),"declaring_type":t.name(),"token":format!("0x{:08x}",token(m,6))}));
            }
            active.remove(&t.pos());
            Ok(())
        }
        visit(self, t, bit, &mut active, &mut slots)?;
        Ok(json!({"name":name,"arch":arch,"slots":slots}))
    }
}
