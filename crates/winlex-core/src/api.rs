use crate::metadata::{Db, supports, token};
use anyhow::{Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};
use std::{
    collections::HashMap,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};
use windows_metadata::{reader::*, *};

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Architecture {
    X86,
    X64,
    Arm64,
}
impl Architecture {
    pub fn bit(self) -> i32 {
        match self {
            Self::X86 => 1,
            Self::X64 => 2,
            Self::Arm64 => 4,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::X86 => "x86",
            Self::X64 => "x64",
            Self::Arm64 => "arm64",
        }
    }
}
impl std::str::FromStr for Architecture {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        Ok(match s {
            "x86" => Self::X86,
            "x64" => Self::X64,
            "arm64" => Self::Arm64,
            _ => bail!("architecture must be x86, x64 or arm64"),
        })
    }
}
#[derive(Clone, Copy, Debug)]
pub enum SearchMode {
    Exact,
    Prefix,
    Contains,
    Integer,
    Guid,
}
pub struct SearchOptions<'a> {
    pub case_sensitive: bool,
    pub mode: SearchMode,
    pub architecture: Option<Architecture>,
    pub offset: usize,
    pub limit: usize,
    pub cancel: Option<&'a AtomicBool>,
}
impl Default for SearchOptions<'_> {
    fn default() -> Self {
        Self {
            case_sensitive: false,
            mode: SearchMode::Contains,
            architecture: None,
            offset: 0,
            limit: 50,
            cancel: None,
        }
    }
}
#[derive(Debug, Serialize)]
pub struct EntitySummary {
    pub token: String,
    pub name: String,
    pub full_name: String,
    pub kind: String,
    pub architecture_mask: i32,
}
#[derive(Debug, Serialize)]
pub struct SearchPage {
    pub total: usize,
    pub offset: usize,
    pub limit: usize,
    pub matches: Vec<EntitySummary>,
}

pub struct Catalog {
    db: Db,
    names: Vec<String>,
}
fn hex(t: u32) -> String {
    format!("0x{t:08x}")
}
fn raw_mask<'a>(r: impl HasAttributes<'a>) -> i32 {
    let a = r.arches();
    if a == 0 { 7 } else { a }
}
fn check_cancel(cancel: Option<&AtomicBool>) -> Result<()> {
    ensure!(
        !cancel.is_some_and(|c| c.load(Ordering::Relaxed)),
        "query cancelled"
    );
    Ok(())
}

pub fn integer(v: &Value) -> Option<i128> {
    Some(match v {
        Value::U8(x) => *x as i128,
        Value::I8(x) => *x as i128,
        Value::U16(x) => *x as i128,
        Value::I16(x) => *x as i128,
        Value::U32(x) => *x as i128,
        Value::I32(x) => *x as i128,
        Value::U64(x) | Value::USize(x) => *x as i128,
        Value::I64(x) | Value::ISize(x) => *x as i128,
        Value::EnumValue(_, v) => return integer(v),
        _ => return None,
    })
}
fn value(v: &Value) -> Json {
    if let Value::EnumValue(n, x) = v {
        return json!({"kind":"enum","name":n.name,"namespace":n.namespace,"value":value(x)});
    }
    if let Some(n) = integer(v) {
        let bits = match v {
            Value::U8(_) | Value::I8(_) => 8,
            Value::U16(_) | Value::I16(_) => 16,
            Value::U32(_) | Value::I32(_) => 32,
            Value::EnumValue(_, x) => return json!({"kind":"enum","value":value(x)}),
            _ => 64,
        };
        return json!({"kind":"integer","type":format!("{:?}",v.ty()),"decimal":n.to_string(),"hex":format!("0x{:0width$x}",n & ((1i128<<bits)-1),width=bits/4),"bits":bits});
    }
    match v {
        Value::Bool(x) => json!({"kind":"boolean","value":x}),
        Value::F32(x) => json!({"kind":"float","type":"F32","value":x.to_string()}),
        Value::F64(x) => json!({"kind":"float","type":"F64","value":x.to_string()}),
        Value::Utf8(x) | Value::Utf16(x) => json!({"kind":"string","value":x}),
        Value::TypeName(n) => json!({"kind":"type_name","name":n.name,"namespace":n.namespace}),
        Value::EnumValue(n, x) => json!({"kind":"enum","name":n.name,"value":value(x)}),
        _ => unreachable!(),
    }
}
fn attributes<'a>(r: impl HasAttributes<'a>) -> Json {
    json!(r.attributes().map(|a|json!({"name":a.name(),"arguments":a.value().iter().map(|(n,v)|json!({"name":n,"value":value(v)})).collect::<Vec<_>>()})).collect::<Vec<_>>())
}
fn guid<'a>(r: impl HasAttributes<'a>) -> Option<String> {
    let a = r.find_attribute("GuidAttribute")?;
    let v = a.value();
    let n: Vec<_> = v.iter().filter_map(|(_, v)| integer(v)).collect();
    if n.len() != 11 {
        return None;
    }
    Some(format!(
        "{:08x}-{:04x}-{:04x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        n[0], n[1], n[2], n[3], n[4], n[5], n[6], n[7], n[8], n[9], n[10]
    ))
}
fn signature(t: &Type) -> Json {
    match t {
        Type::ClassName(n) | Type::ValueName(n) => {
            json!({"kind":"named","name":n.name,"namespace":n.namespace,"reference_token":n.identity.map(|(_,t)|hex(t)),"arguments":n.generics.iter().map(signature).collect::<Vec<_>>()})
        }
        Type::PtrMut(t, d) => {
            json!({"kind":"pointer","depth":d,"const":false,"element":signature(t)})
        }
        Type::PtrConst(t, d) => {
            json!({"kind":"pointer","depth":d,"const":true,"element":signature(t)})
        }
        Type::ArrayFixed(t, n) => json!({"kind":"array","length":n,"element":signature(t)}),
        Type::Array(t) => json!({"kind":"array","length":null,"element":signature(t)}),
        Type::RefMut(t) => json!({"kind":"byref","element":signature(t)}),
        Type::RefConst(t) => json!({"kind":"const","element":signature(t)}),
        Type::Generic(n, i) => json!({"kind":"generic","name":n,"index":i}),
        other => json!({"kind":"primitive","name":format!("{other:?}")}),
    }
}

impl Catalog {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let db = Db::open(path.as_ref())?;
        let parents: HashMap<_, _> = db
            .index
            .rows::<NestedClass>(0)
            .map(|n| (n.usize(0) - 1, n.usize(1) - 1))
            .collect();
        fn full(
            db: &Db,
            p: usize,
            parents: &HashMap<usize, usize>,
            depth: usize,
        ) -> Result<String> {
            ensure!(depth < 128, "nested type depth exceeds limit");
            let t = db.ty(p);
            Ok(if let Some(parent) = parents.get(&p) {
                format!("{}+{}", full(db, *parent, parents, depth + 1)?, t.name())
            } else if t.namespace().is_empty() {
                t.name().into()
            } else {
                format!("{}.{}", t.namespace(), t.name())
            })
        }
        let names = (0..db.type_count)
            .map(|p| full(&db, p, &parents, 0))
            .collect::<Result<Vec<_>>>()?;
        Ok(Self { db, names })
    }
    pub fn source_sha256(&self) -> &str {
        &self.db.sha
    }
    pub fn summary(&self) -> Json {
        json!({"sha256":self.db.sha,"type_rows":self.db.type_count,"field_rows":self.db.field_count,"method_rows":self.db.method_count})
    }
    fn type_name(&self, t: TypeDef<'_>) -> String {
        self.names[t.pos()].clone()
    }
    fn type_summary(&self, t: TypeDef<'_>) -> EntitySummary {
        EntitySummary {
            token: hex(token(t, 2)),
            name: t.name().into(),
            full_name: self.type_name(t),
            kind: if t.name() == "<Module>" {
                "module".into()
            } else {
                format!("{:?}", t.category()).to_lowercase()
            },
            architecture_mask: raw_mask(t),
        }
    }
    fn member_summary(
        &self,
        t: TypeDef<'_>,
        name: &str,
        tok: u32,
        kind: &str,
        mask: i32,
    ) -> EntitySummary {
        EntitySummary {
            token: hex(tok),
            name: name.into(),
            full_name: format!("{}.{}", self.type_name(t), name),
            kind: kind.into(),
            architecture_mask: raw_mask(t) & mask,
        }
    }
    pub fn search(&self, q: &str, o: SearchOptions<'_>) -> Result<SearchPage> {
        ensure!(o.limit <= 1000, "limit must be <= 1000");
        ensure!(!q.is_empty(), "empty search query");
        let q = if o.case_sensitive {
            q.to_owned()
        } else {
            q.to_lowercase()
        };
        let num = if matches!(o.mode, SearchMode::Integer) {
            Some(if let Some(h) = q.strip_prefix("0x") {
                i128::from_str_radix(h, 16)?
            } else {
                q.parse()?
            })
        } else {
            None
        };
        let g = q.trim_matches(['{', '}']);
        if matches!(o.mode, SearchMode::Guid) {
            ensure!(
                g.len() == 36
                    && [8, 13, 18, 23].iter().all(|i| g.as_bytes()[*i] == b'-')
                    && g.bytes()
                        .enumerate()
                        .all(|(i, c)| [8, 13, 18, 23].contains(&i) || c.is_ascii_hexdigit()),
                "invalid GUID"
            );
        }
        let mut page = SearchPage {
            total: 0,
            offset: o.offset,
            limit: o.limit,
            matches: vec![],
        };
        let mut accept = |s: EntitySummary, number: Option<i128>, guid: Option<String>| {
            if o.architecture
                .is_some_and(|a| s.architecture_mask & a.bit() == 0)
            {
                return;
            }
            let name = if o.case_sensitive {
                s.name.clone()
            } else {
                s.name.to_lowercase()
            };
            let full = if o.case_sensitive {
                s.full_name.clone()
            } else {
                s.full_name.to_lowercase()
            };
            let matched = match o.mode {
                SearchMode::Exact => name == q || full == q,
                SearchMode::Prefix => name.starts_with(&q) || full.starts_with(&q),
                SearchMode::Contains => name.contains(&q) || full.contains(&q),
                SearchMode::Integer => number.is_some() && number == num,
                SearchMode::Guid => guid.as_deref() == Some(g),
            };
            if matched {
                if page.total >= o.offset && page.matches.len() < o.limit {
                    page.matches.push(s);
                }
                page.total += 1;
            }
        };
        for p in 0..self.db.type_count {
            check_cancel(o.cancel)?;
            let t = self.db.ty(p);
            if matches!(o.mode, SearchMode::Integer) {
            } else {
                accept(
                    self.type_summary(t),
                    None,
                    if matches!(o.mode, SearchMode::Guid) {
                        guid(t)
                    } else {
                        None
                    },
                );
            }
            for f in t.fields() {
                accept(
                    self.member_summary(
                        t,
                        f.name(),
                        token(f, 4),
                        if f.constant().is_some() {
                            "constant"
                        } else {
                            "field"
                        },
                        raw_mask(f),
                    ),
                    if matches!(o.mode, SearchMode::Integer) {
                        f.constant().and_then(|c| integer(&c.value()))
                    } else {
                        None
                    },
                    if matches!(o.mode, SearchMode::Guid) {
                        guid(f)
                    } else {
                        None
                    },
                );
            }
            if !matches!(o.mode, SearchMode::Integer | SearchMode::Guid) {
                for m in t.methods() {
                    accept(
                        self.member_summary(
                            t,
                            m.name(),
                            token(m, 6),
                            if t.name() == "Apis" {
                                "function"
                            } else {
                                "method"
                            },
                            raw_mask(m),
                        ),
                        None,
                        None,
                    );
                }
            }
        }
        Ok(page)
    }
    fn select(&self, s: &str, arch: Option<Architecture>) -> Result<u32> {
        if let Some(h) = s.strip_prefix("0x") {
            return Ok(u32::from_str_radix(h, 16)?);
        }
        let mut p = self.search(
            s,
            SearchOptions {
                mode: SearchMode::Exact,
                architecture: arch,
                limit: 10,
                case_sensitive: true,
                ..Default::default()
            },
        )?;
        if p.total == 0 {
            p = self.search(
                s,
                SearchOptions {
                    mode: SearchMode::Exact,
                    architecture: arch,
                    limit: 10,
                    ..Default::default()
                },
            )?;
        }
        ensure!(
            p.total == 1,
            "expected one entity, found {}; specify --arch, a qualified name or token. Candidates: {}",
            p.total,
            p.matches
                .iter()
                .map(|m| format!("{} {}", m.token, m.full_name))
                .collect::<Vec<_>>()
                .join(", ")
        );
        Ok(u32::from_str_radix(
            p.matches[0].token.trim_start_matches("0x"),
            16,
        )?)
    }
    pub fn describe(&self, s: &str, arch: Option<Architecture>) -> Result<Json> {
        self.describe_token(self.select(s, arch)?, arch)
    }
    fn select_type(&self, s: &str, a: Architecture) -> Result<u32> {
        if s.starts_with("0x") {
            let t = self.select(s, Some(a))?;
            ensure!(t >> 24 == 2, "expected a TypeDef token");
            return Ok(t);
        }
        let mut found: Vec<_> = (0..self.db.type_count)
            .map(|p| self.db.ty(p))
            .filter(|t| {
                supports(*t, a.bit())
                    && (t.name().eq_ignore_ascii_case(s)
                        || self.names[t.pos()].eq_ignore_ascii_case(s))
            })
            .collect();
        if found
            .iter()
            .any(|t| t.name() == s || self.names[t.pos()] == s)
        {
            found.retain(|t| t.name() == s || self.names[t.pos()] == s);
        }
        ensure!(
            found.len() == 1,
            "expected one type, found {}; use qualified name or token",
            found.len()
        );
        Ok(token(found[0], 2))
    }
    fn describe_token(&self, tok: u32, arch: Option<Architecture>) -> Result<Json> {
        let pos = (tok & 0xffffff)
            .checked_sub(1)
            .ok_or_else(|| anyhow::anyhow!("nil token"))? as usize;
        let row = match tok >> 24 {
            2 if pos < self.db.type_count => {
                let t = self.db.ty(pos);
                ensure!(
                    arch.is_none_or(|a| supports(t, a.bit())),
                    "type unavailable on architecture"
                );
                json!({"entity":self.type_summary(t),"flags":t.flags().0,"guid":guid(t),"attributes":attributes(t),"layout_constraints":t.class_layout().map(|l|json!({"pack_bytes":l.packing_size(),"declared_size_bytes":l.class_size()})),"layout_kind":if t.flags().contains(TypeAttributes::ExplicitLayout){"explicit"}else if t.flags().contains(TypeAttributes::SequentialLayout){"sequential"}else{"auto"},
                "interfaces":t.interface_impls().map(|i|signature(&i.interface(&[]))).collect::<Vec<_>>(),"fields":t.fields().filter(|f|arch.is_none_or(|a|supports(*f,a.bit()))).map(|f|self.describe_token(token(f,4),arch)).collect::<Result<Vec<_>>>()?,"methods":t.methods().filter(|m|arch.is_none_or(|a|supports(*m,a.bit()))).map(|m|self.describe_token(token(m,6),arch)).collect::<Result<Vec<_>>>()?})
            }
            4 if pos < self.db.field_count => {
                let f = self.db.field(pos);
                let parent: TypeDef = f.parent_row(4);
                ensure!(
                    arch.is_none_or(|a| supports(parent, a.bit()) && supports(f, a.bit())),
                    "field unavailable on architecture"
                );
                json!({"entity":self.member_summary(parent,f.name(),tok,if f.constant().is_some(){"constant"}else{"field"},raw_mask(f)),"parent_token":hex(token(parent,2)),"type":signature(&f.ty()),"declared_offset_bytes":f.explicit_offset(),"bitfields":crate::metadata::bitfields(f)?,"value":f.constant().map(|c|value(&c.value())),"guid":guid(f),"attributes":attributes(f)})
            }
            6 if pos < self.db.method_count => {
                let m = self.db.method(pos);
                let parent: TypeDef = m.parent_row(5);
                let s = m.signature(&[]);
                let ps = m.params_by_sequence(s.types.len())?;
                ensure!(
                    arch.is_none_or(|a| supports(parent, a.bit()) && supports(m, a.bit())),
                    "method unavailable on architecture"
                );
                json!({"entity":self.member_summary(parent,m.name(),tok,if parent.name()=="Apis"{"function"}else{"method"},raw_mask(m)),"parent_token":hex(token(parent,2)),"return_type":signature(&s.return_type),"signature_flags":s.flags.0,"return_attributes":ps.return_param().map(attributes),"attributes":attributes(m),"import":m.impl_map().map(|i|json!({"dll":i.import_scope().name(),"entry_point":i.import_name(),"flags":i.flags().0,"calling_convention":m.calling_convention()})),
                "parameters":s.types.iter().zip(ps.params()).enumerate().map(|(i,(t,p))|json!({"sequence":i+1,"name":p.map(|p|p.name().to_owned()),"type":signature(t),"flags":p.map(|p|p.flags().0),"direction":p.map(|p|format!("{:?}",p.direction())),"optional":p.map(|p|p.is_optional()),"attributes":p.map(attributes)})).collect::<Vec<_>>()})
            }
            1 if pos < self.db.index.rows::<TypeRef>(0).len() => {
                let r = TypeRef::from_row(Row {
                    index: &self.db.index,
                    file: 0,
                    pos,
                });
                let n = TypeDefOrRef::TypeRef(r).type_name();
                let resolution = arch.map(|a| match self.db.resolve(&n, self.db.ty(0), a.bit()) {
                    Ok(t) => json!({"target_token":hex(token(t,2))}),
                    Err(e) => json!({"unresolved":e.to_string()}),
                });
                json!({"entity":{"token":hex(tok),"kind":"type_reference","name":r.name(),"namespace":r.namespace()},"scope_token":hex(r.scope_token()),"resolution":resolution})
            }
            _ => bail!("unsupported or out-of-range token {}", hex(tok)),
        };
        Ok(row)
    }
    pub fn layout(&self, s: &str, a: Architecture) -> Result<Json> {
        let tok = self.select_type(s, a)?;
        crate::layout::derive_token(&self.db, tok, a.name())
    }
    pub fn vtable(&self, s: &str, a: Architecture) -> Result<Json> {
        let tok = self.select_type(s, a)?;
        let mut result = self.db.vtable_token(tok, a.name())?;
        if let Some(slots) = result["slots"].as_array_mut() {
            for slot in slots {
                slot["byte_offset"] = json!(
                    slot["index"].as_u64().unwrap()
                        * if matches!(a, Architecture::X86) { 4 } else { 8 }
                );
            }
        }
        Ok(result)
    }
    /// Reverse type uses. A member is reported once even if several parameters use it.
    pub fn references(&self, s: &str, a: Architecture, o: SearchOptions<'_>) -> Result<Json> {
        ensure!(o.limit <= 1000, "limit must be <= 1000");
        let target = self.select_type(s, a)?;
        fn uses(
            db: &Db,
            t: &Type,
            owner: TypeDef<'_>,
            arch: i32,
            target: u32,
            unresolved: &mut usize,
        ) -> bool {
            match t {
                Type::ClassName(n) | Type::ValueName(n) => {
                    let direct = match db.resolve(n, owner, arch) {
                        Ok(t) => token(t, 2) == target,
                        Err(_) => {
                            *unresolved += 1;
                            false
                        }
                    };
                    n.generics.iter().fold(direct, |found, t| {
                        uses(db, t, owner, arch, target, unresolved) || found
                    })
                }
                Type::Array(t)
                | Type::ArrayFixed(t, _)
                | Type::PtrMut(t, _)
                | Type::PtrConst(t, _)
                | Type::RefMut(t)
                | Type::RefConst(t) => uses(db, t, owner, arch, target, unresolved),
                _ => false,
            }
        }
        let mut page = SearchPage {
            total: 0,
            offset: o.offset,
            limit: o.limit,
            matches: vec![],
        };
        let mut unresolved = 0;
        let mut add = |s: EntitySummary| {
            if page.total >= o.offset && page.matches.len() < o.limit {
                page.matches.push(s);
            }
            page.total += 1;
        };
        for p in 0..self.db.type_count {
            check_cancel(o.cancel)?;
            let t = self.db.ty(p);
            if !supports(t, a.bit()) {
                continue;
            }
            for f in t.fields().filter(|f| supports(*f, a.bit())) {
                if uses(&self.db, &f.ty(), t, a.bit(), target, &mut unresolved) {
                    add(self.member_summary(t, f.name(), token(f, 4), "field", raw_mask(f)));
                }
            }
            for m in t.methods().filter(|m| supports(*m, a.bit())) {
                let sig = m.signature(&[]);
                let mut hit = uses(
                    &self.db,
                    &sig.return_type,
                    t,
                    a.bit(),
                    target,
                    &mut unresolved,
                );
                for ty in &sig.types {
                    hit = uses(&self.db, ty, t, a.bit(), target, &mut unresolved) || hit;
                }
                if hit {
                    add(self.member_summary(t, m.name(), token(m, 6), "method", raw_mask(m)));
                }
            }
            for i in t.interface_impls() {
                if uses(
                    &self.db,
                    &i.interface(&[]),
                    t,
                    a.bit(),
                    target,
                    &mut unresolved,
                ) {
                    add(self.type_summary(t));
                    break;
                }
            }
        }
        Ok(
            json!({"target_token":hex(target),"architecture":a,"page":page,"unresolved_type_uses":unresolved,"scope":"field/method signatures and interface bases; attribute-only and macro relations not included"}),
        )
    }
}
