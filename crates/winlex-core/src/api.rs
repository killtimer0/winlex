use crate::metadata::{Db, supports, token};
use crate::source::{Key, Source};
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
    pub source: Option<&'a str>,
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
            source: None,
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
    pub id: String,
    pub source_id: String,
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
    names: HashMap<Key, String>,
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
fn signature(t: &Type, db: &Db) -> Json {
    match t {
        Type::ClassName(n) | Type::ValueName(n) => {
            json!({"kind":"named","name":n.name,"namespace":n.namespace,"reference_token":n.identity.map(|(_,t)|hex(t)),"reference_id":n.identity.map(|(file,token)|db.id(Key { file, token })),"arguments":n.generics.iter().map(|t|signature(t,db)).collect::<Vec<_>>()})
        }
        Type::PtrMut(t, d) => {
            json!({"kind":"pointer","depth":d,"const":false,"element":signature(t,db)})
        }
        Type::PtrConst(t, d) => {
            json!({"kind":"pointer","depth":d,"const":true,"element":signature(t,db)})
        }
        Type::ArrayFixed(t, n) => json!({"kind":"array","length":n,"element":signature(t,db)}),
        Type::Array(t) => json!({"kind":"array","length":null,"element":signature(t,db)}),
        Type::RefMut(t) => json!({"kind":"byref","element":signature(t,db)}),
        Type::RefConst(t) => json!({"kind":"const","element":signature(t,db)}),
        Type::Generic(n, i) => json!({"kind":"generic","scope":"type","name":n,"index":i}),
        Type::MethodGeneric(n, i) => json!({"kind":"generic","scope":"method","name":n,"index":i}),
        other => json!({"kind":"primitive","name":format!("{other:?}")}),
    }
}

impl Catalog {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::open_many([path])
    }
    pub fn open_many<P: AsRef<Path>>(paths: impl IntoIterator<Item = P>) -> Result<Self> {
        let db = Db::open_many(paths)?;
        let parents: HashMap<_, _> = (0..db.sources.len())
            .flat_map(|file| {
                db.index
                    .rows::<NestedClass>(file)
                    .map(move |n| ((file, n.usize(0) - 1), n.usize(1) - 1))
            })
            .collect();
        fn full(
            db: &Db,
            t: TypeDef<'_>,
            parents: &HashMap<(usize, usize), usize>,
            depth: usize,
        ) -> Result<String> {
            ensure!(depth < 128, "nested type depth exceeds limit");
            let file = t.to_row().file;
            Ok(if let Some(parent) = parents.get(&(file, t.pos())) {
                let p = db.row::<TypeDef>(
                    Key {
                        file,
                        token: 0x02000000 | (*parent as u32 + 1),
                    },
                    2,
                )?;
                format!("{}+{}", full(db, p, parents, depth + 1)?, t.name())
            } else if t.namespace().is_empty() {
                t.name().into()
            } else {
                format!("{}.{}", t.namespace(), t.name())
            })
        }
        let names = (0..db.type_count)
            .map(|p| {
                let t = db.ty(p);
                Ok((db.key(t, 2), full(&db, t, &parents, 0)?))
            })
            .collect::<Result<HashMap<_, _>>>()?;
        Ok(Self { db, names })
    }
    pub fn source_sha256(&self) -> Option<&str> {
        (self.db.sources.len() == 1).then_some(self.db.sources[0].id.as_str())
    }
    pub fn sources(&self) -> &[Source] {
        &self.db.sources
    }
    pub fn summary(&self) -> Json {
        json!({"sources":self.sources(),"type_rows":self.db.type_count,"field_rows":self.db.field_count,"method_rows":self.db.method_count})
    }
    fn type_name(&self, t: TypeDef<'_>) -> String {
        self.names[&self.db.key(t, 2)].clone()
    }
    fn type_summary(&self, t: TypeDef<'_>) -> EntitySummary {
        EntitySummary {
            id: self.db.id(self.db.key(t, 2)),
            source_id: self.db.sources[t.to_row().file].id.clone(),
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
            id: self.db.id(Key {
                file: t.to_row().file,
                token: tok,
            }),
            source_id: self.db.sources[t.to_row().file].id.clone(),
            token: hex(tok),
            name: name.into(),
            full_name: format!("{}.{}", self.type_name(t), name),
            kind: kind.into(),
            architecture_mask: raw_mask(t) & mask,
        }
    }
    pub fn search(&self, q: &str, o: SearchOptions<'_>) -> Result<SearchPage> {
        let source_file = o.source.map(|s| self.db.source_file(s)).transpose()?;
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
            if source_file.is_some_and(|f| f != t.to_row().file) {
                continue;
            }
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
                for p in t.properties() {
                    accept(
                        self.member_summary(t, p.name(), token(p, 0x17), "property", raw_mask(p)),
                        None,
                        None,
                    );
                }
                for e in t.events() {
                    accept(
                        self.member_summary(t, e.name(), token(e, 0x14), "event", raw_mask(e)),
                        None,
                        None,
                    );
                }
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
    fn selector<'a>(&self, s: &'a str) -> Result<(Option<usize>, &'a str)> {
        if let Some((source, name)) = s.split_once("::") {
            Ok((Some(self.db.source_file(source)?), name))
        } else {
            Ok((None, s))
        }
    }
    fn select(&self, s: &str, arch: Option<Architecture>) -> Result<Key> {
        let (file, s) = self.selector(s)?;
        if let Some(h) = s.strip_prefix("0x") {
            ensure!(
                file.is_some() || self.sources().len() == 1,
                "token requires SOURCE::TOKEN with multiple sources"
            );
            return Ok(Key {
                file: file.unwrap_or(0),
                token: u32::from_str_radix(h, 16)?,
            });
        }
        let source = file.map(|f| self.db.sources[f].id.as_str());
        let mut p = self.search(
            s,
            SearchOptions {
                source,
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
                    source,
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
                .map(|m| format!("{} {}", m.id, m.full_name))
                .collect::<Vec<_>>()
                .join(", ")
        );
        Ok(Key {
            file: self.db.source_file(&p.matches[0].source_id)?,
            token: u32::from_str_radix(p.matches[0].token.trim_start_matches("0x"), 16)?,
        })
    }
    pub fn describe(&self, s: &str, arch: Option<Architecture>) -> Result<Json> {
        self.describe_token(self.select(s, arch)?, arch)
    }
    fn select_type(&self, selector: &str, a: Architecture) -> Result<Key> {
        let (file, s) = self.selector(selector)?;
        if s.starts_with("0x") {
            let t = self.select(selector, Some(a))?;
            self.db.row::<TypeDef>(t, 2)?;
            return Ok(t);
        }
        let mut found: Vec<_> = (0..self.db.type_count)
            .map(|p| self.db.ty(p))
            .filter(|t| {
                supports(*t, a.bit())
                    && file.is_none_or(|f| f == t.to_row().file)
                    && (t.name().eq_ignore_ascii_case(s)
                        || self.type_name(*t).eq_ignore_ascii_case(s))
            })
            .collect();
        if found
            .iter()
            .any(|t| t.name() == s || self.type_name(*t) == s)
        {
            found.retain(|t| t.name() == s || self.type_name(*t) == s);
        }
        ensure!(
            found.len() == 1,
            "expected one type, found {}; use SOURCE::NAME or SOURCE::TOKEN. Candidates: {}",
            found.len(),
            found
                .iter()
                .take(10)
                .map(|t| self.db.id(self.db.key(*t, 2)))
                .collect::<Vec<_>>()
                .join(", ")
        );
        Ok(self.db.key(found[0], 2))
    }
    fn describe_token(&self, key: Key, arch: Option<Architecture>) -> Result<Json> {
        let tok = key.token;
        let file = key.file;
        let row = match tok >> 24 {
            2 => {
                let t: TypeDef = self.db.row(key, 2)?;
                ensure!(
                    arch.is_none_or(|a| supports(t, a.bit())),
                    "type unavailable on architecture"
                );
                let mut result = json!({"entity":self.type_summary(t),"flags":t.flags().0,"guid":guid(t),"attributes":attributes(t),"generic_parameters":t.generic_params().map(|p|json!({"name":p.name(),"index":p.sequence(),"flags":p.flags().0})).collect::<Vec<_>>(),"layout_constraints":t.class_layout().map(|l|json!({"pack_bytes":l.packing_size(),"declared_size_bytes":l.class_size()})),"layout_kind":if t.flags().contains(TypeAttributes::ExplicitLayout){"explicit"}else if t.flags().contains(TypeAttributes::SequentialLayout){"sequential"}else{"auto"},
                "fields":t.fields().filter(|f|arch.is_none_or(|a|supports(*f,a.bit()))).map(|f|self.describe_token(self.db.key(f,4),arch)).collect::<Result<Vec<_>>>()?,"methods":t.methods().filter(|m|arch.is_none_or(|a|supports(*m,a.bit()))).map(|m|self.describe_token(self.db.key(m,6),arch)).collect::<Result<Vec<_>>>()?});
                result["properties"] = json!(
                    t.properties()
                        .filter(|p| arch.is_none_or(|a| supports(*p, a.bit())))
                        .map(|p| self.describe_token(self.db.key(p, 0x17), arch))
                        .collect::<Result<Vec<_>>>()?
                );
                result["events"] = json!(
                    t.events()
                        .filter(|e| arch.is_none_or(|a| supports(*e, a.bit())))
                        .map(|e| self.describe_token(self.db.key(e, 0x14), arch))
                        .collect::<Result<Vec<_>>>()?
                );
                result["interface_implementations"] = json!(t.interface_impls().map(|i|json!({"type":signature(&i.interface(&t.generic_types()),&self.db),"default":i.has_attribute("DefaultAttribute"),"attributes":attributes(i)})).collect::<Vec<_>>());
                result["base_type"] = t
                    .extends()
                    .map(|b| {
                        let ty = match b {
                            TypeDefOrRef::TypeSpec(s) => s.ty(&t.generic_types()),
                            b => Type::ClassName(b.type_name()),
                        };
                        signature(&ty, &self.db)
                    })
                    .unwrap_or(Json::Null);
                result["windows_runtime"] = json!(t.flags().0 & 0x4000 != 0);
                result
            }
            0x17 => {
                let p: Property = self.db.row(key, 0x17)?;
                let parent = p.parent();
                ensure!(
                    arch.is_none_or(|a| supports(parent, a.bit()) && supports(p, a.bit())),
                    "property unavailable on architecture"
                );
                let s = p.signature();
                json!({"entity":self.member_summary(parent,p.name(),tok,"property",raw_mask(p)),"parent_id":self.db.id(self.db.key(parent,2)),"flags":p.flags(),"type":signature(&s.return_type,&self.db),"index_parameters":s.types.iter().map(|t|signature(t,&self.db)).collect::<Vec<_>>(),"attributes":attributes(p),"accessors":p.accessors().map(|s|json!({"semantics":s.flags(),"method_id":self.db.id(self.db.key(s.method(),6)),"name":s.method().name()})).collect::<Vec<_>>()})
            }
            0x14 => {
                let e: Event = self.db.row(key, 0x14)?;
                let parent = e.parent();
                ensure!(
                    arch.is_none_or(|a| supports(parent, a.bit()) && supports(e, a.bit())),
                    "event unavailable on architecture"
                );
                json!({"entity":self.member_summary(parent,e.name(),tok,"event",raw_mask(e)),"parent_id":self.db.id(self.db.key(parent,2)),"flags":e.flags(),"type":signature(&e.ty(),&self.db),"attributes":attributes(e),"accessors":e.accessors().map(|s|json!({"semantics":s.flags(),"method_id":self.db.id(self.db.key(s.method(),6)),"name":s.method().name()})).collect::<Vec<_>>()})
            }
            4 => {
                let f: Field = self.db.row(key, 4)?;
                let parent: TypeDef = f.parent_row(4);
                ensure!(
                    arch.is_none_or(|a| supports(parent, a.bit()) && supports(f, a.bit())),
                    "field unavailable on architecture"
                );
                json!({"entity":self.member_summary(parent,f.name(),tok,if f.constant().is_some(){"constant"}else{"field"},raw_mask(f)),"parent_token":hex(token(parent,2)),"parent_id":self.db.id(self.db.key(parent,2)),"type":signature(&f.ty(),&self.db),"declared_offset_bytes":f.explicit_offset(),"bitfields":crate::metadata::bitfields(f)?,"value":f.constant().map(|c|value(&c.value())),"guid":guid(f),"attributes":attributes(f)})
            }
            6 => {
                let m: MethodDef = self.db.row(key, 6)?;
                let parent: TypeDef = m.parent_row(5);
                let s = m.signature(&[]);
                let ps = m.params_by_sequence(s.types.len())?;
                ensure!(
                    arch.is_none_or(|a| supports(parent, a.bit()) && supports(m, a.bit())),
                    "method unavailable on architecture"
                );
                json!({"entity":self.member_summary(parent,m.name(),tok,if parent.name()=="Apis"{"function"}else{"method"},raw_mask(m)),"parent_token":hex(token(parent,2)),"parent_id":self.db.id(self.db.key(parent,2)),"return_type":signature(&s.return_type,&self.db),"signature_flags":s.flags.0,"return_attributes":ps.return_param().map(attributes),"attributes":attributes(m),"import":m.impl_map().map(|i|json!({"dll":i.import_scope().name(),"entry_point":i.import_name(),"flags":i.flags().0,"calling_convention":m.calling_convention()})),
                "generic_parameters":m.generic_params().map(|p|json!({"name":p.name(),"index":p.sequence(),"flags":p.flags().0})).collect::<Vec<_>>(),"parameters":s.types.iter().zip(ps.params()).enumerate().map(|(i,(t,p))|json!({"sequence":i+1,"name":p.map(|p|p.name().to_owned()),"type":signature(t,&self.db),"flags":p.map(|p|p.flags().0),"direction":p.map(|p|format!("{:?}",p.direction())),"optional":p.map(|p|p.is_optional()),"attributes":p.map(attributes)})).collect::<Vec<_>>()})
            }
            1 => {
                let r: TypeRef = self.db.row(key, 1)?;
                let n = TypeDefOrRef::TypeRef(r).type_name();
                let owner: TypeDef = self.db.row(
                    Key {
                        file,
                        token: 0x02000001,
                    },
                    2,
                )?;
                let resolution = arch.map(|a| match self.db.resolve(&n, owner, a.bit()) {
                    Ok(t) => json!({"target_token":hex(token(t,2)),"target_id":self.db.id(self.db.key(t,2)),"target_source_id":self.db.sources[t.to_row().file].id}),
                    Err(e) => json!({"unresolved":e.to_string()}),
                });
                json!({"entity":{"id":self.db.id(key),"source_id":self.db.sources[file].id,"token":hex(tok),"kind":"type_reference","name":r.name(),"namespace":r.namespace()},"scope_token":hex(r.scope_token()),"resolution":resolution})
            }
            _ => bail!("unsupported or out-of-range token {}", hex(tok)),
        };
        Ok(row)
    }
    pub fn layout(&self, s: &str, a: Architecture) -> Result<Json> {
        let tok = self.select_type(s, a)?;
        crate::layout::derive_key(&self.db, tok, a.name())
    }
    pub fn vtable(&self, s: &str, a: Architecture) -> Result<Json> {
        let tok = self.select_type(s, a)?;
        let mut result = self.db.vtable_key(tok, a.name())?;
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
        let source_file = o.source.map(|s| self.db.source_file(s)).transpose()?;
        let target = self.select_type(s, a)?;
        fn uses(
            db: &Db,
            t: &Type,
            owner: TypeDef<'_>,
            arch: i32,
            target: Key,
            unresolved: &mut usize,
        ) -> bool {
            match t {
                Type::ClassName(n) | Type::ValueName(n) => {
                    let mut definition = n.clone();
                    definition.generics.clear();
                    let direct = match db.resolve(&definition, owner, arch) {
                        Ok(t) => db.key(t, 2) == target,
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
            if !supports(t, a.bit()) || source_file.is_some_and(|f| f != t.to_row().file) {
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
                    &i.interface(&t.generic_types()),
                    t,
                    a.bit(),
                    target,
                    &mut unresolved,
                ) {
                    add(self.type_summary(t));
                    break;
                }
            }
            for p in t.properties().filter(|p| supports(*p, a.bit())) {
                let sig = p.signature();
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
                    add(self.member_summary(t, p.name(), token(p, 0x17), "property", raw_mask(p)));
                }
            }
            for e in t.events().filter(|e| supports(*e, a.bit())) {
                if uses(&self.db, &e.ty(), t, a.bit(), target, &mut unresolved) {
                    add(self.member_summary(t, e.name(), token(e, 0x14), "event", raw_mask(e)));
                }
            }
        }
        Ok(
            json!({"target_token":hex(target.token),"target_id":self.db.id(target),"target_source_id":self.db.sources[target.file].id,"architecture":a,"page":page,"unresolved_type_uses":unresolved,"scope":"field/method/property/event signatures and interface requirements"}),
        )
    }
}
