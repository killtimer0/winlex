use crate::{
    metadata::{Db, key, supports},
    model::*,
    source::{EntityId, Source},
};
use anyhow::{Result, bail, ensure};
use std::{
    collections::HashMap,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};
use windows_metadata::{reader::*, *};

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Architecture {
    X86,
    X64,
    Arm64,
}
impl Architecture {
    pub const ALL_MASK: i32 = Self::X86.bit() | Self::X64.bit() | Self::Arm64.bit();

    pub const fn bit(self) -> i32 {
        match self {
            Self::X86 => 1,
            Self::X64 => 2,
            Self::Arm64 => 4,
        }
    }
    pub fn supports(self, mask: i32) -> bool {
        mask & self.bit() != 0
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
#[derive(Clone, Copy, Debug, Default)]
pub enum SearchMode {
    Exact,
    Prefix,
    #[default]
    Contains,
    Integer,
    Guid,
}
#[derive(Default)]
pub struct SearchOptions<'a> {
    pub source: Option<usize>,
    pub case_sensitive: bool,
    pub mode: SearchMode,
    pub architecture: Option<Architecture>,
    pub types_only: bool,
    pub cancel: Option<&'a AtomicBool>,
}
#[derive(Debug, serde::Serialize)]
pub struct EntitySummary {
    pub id: EntityId,
    pub name: String,
    pub full_name: String,
    pub kind: String,
    pub architecture_mask: i32,
}
pub struct Catalog {
    db: Db,
    names: HashMap<EntityId, String>,
    search_index: Vec<EntityId>,
}
fn raw_mask<'a>(r: impl HasAttributes<'a>) -> i32 {
    let a = r.arches();
    if a == 0 { Architecture::ALL_MASK } else { a }
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
pub fn guid<'a>(r: impl HasAttributes<'a>) -> Option<String> {
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
impl<'a> Declaration<'a> {
    pub fn name(&self) -> &'a str {
        match self {
            Self::Type(r) => r.name(),
            Self::Field(r) => r.name(),
            Self::Method(r) => r.name(),
            Self::Property(r) => r.name(),
            Self::Event(r) => r.name(),
            Self::Reference(r) => r.name(),
        }
    }
    fn owner(&self) -> Option<TypeDef<'a>> {
        Some(match self {
            Self::Type(r) => *r,
            Self::Field(r) => r.parent_row(4),
            Self::Method(r) => r.parent_row(5),
            Self::Property(r) => r.parent(),
            Self::Event(r) => r.parent(),
            Self::Reference(_) => return None,
        })
    }
    pub fn architecture_mask(&self) -> i32 {
        let member = match self {
            Self::Type(r) => return raw_mask(*r),
            Self::Field(r) => raw_mask(*r),
            Self::Method(r) => raw_mask(*r),
            Self::Property(r) => raw_mask(*r),
            Self::Event(r) => raw_mask(*r),
            Self::Reference(_) => Architecture::ALL_MASK,
        };
        member & self.owner().map(raw_mask).unwrap_or(Architecture::ALL_MASK)
    }
}
impl Catalog {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::open_many([path])
    }
    pub fn open_many<P: AsRef<Path>>(paths: impl IntoIterator<Item = P>) -> Result<Self> {
        Self::from_inputs(
            paths
                .into_iter()
                .map(crate::SourceData::read)
                .collect::<Result<Vec<_>>>()?,
        )
    }
    pub fn inputs(&self) -> &[crate::SourceData] {
        &self.db.inputs
    }
    pub fn sources(&self) -> &[Source] {
        &self.db.sources
    }
    pub fn row_counts(&self) -> (usize, usize, usize) {
        (0..self.sources().len()).fold((0, 0, 0), |(types, fields, methods), file| {
            (
                types + self.db.index.rows::<TypeDef>(file).len(),
                fields + self.db.index.rows::<Field>(file).len(),
                methods + self.db.index.rows::<MethodDef>(file).len(),
            )
        })
    }
    /// All type definitions in source/token order, including module rows.
    pub fn type_ids(&self) -> impl Iterator<Item = EntityId> + '_ {
        self.search_index
            .iter()
            .copied()
            .filter(|id| id.token >> 24 == 2)
    }
    pub fn from_inputs(inputs: impl IntoIterator<Item = crate::SourceData>) -> Result<Self> {
        let db = Db::from_inputs(inputs)?;
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
                    EntityId {
                        source_id: file,
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
        let mut names = HashMap::new();
        let mut search_index = Vec::new();
        for file in 0..db.sources.len() {
            for t in db.index.rows::<TypeDef>(file) {
                names.insert(key(t, 2), full(&db, t, &parents, 0)?);
                search_index.push(key(t, 2));
                search_index.extend(t.fields().map(|r| key(r, 4)));
                search_index.extend(t.methods().map(|r| key(r, 6)));
                search_index.extend(t.events().map(|r| key(r, 0x14)));
                search_index.extend(t.properties().map(|r| key(r, 0x17)));
            }
        }
        search_index.sort_unstable();
        Ok(Self {
            db,
            names,
            search_index,
        })
    }
    pub fn declaration(&self, id: EntityId, arch: Option<Architecture>) -> Result<Declaration<'_>> {
        let row = match id.token >> 24 {
            1 => Declaration::Reference(self.db.row(id, 1)?),
            2 => Declaration::Type(self.db.row(id, 2)?),
            4 => Declaration::Field(self.db.row(id, 4)?),
            6 => Declaration::Method(self.db.row(id, 6)?),
            0x14 => Declaration::Event(self.db.row(id, 0x14)?),
            0x17 => Declaration::Property(self.db.row(id, 0x17)?),
            _ => bail!("unsupported token {id}"),
        };
        ensure!(
            arch.is_none_or(|a| row.architecture_mask() & a.bit() != 0),
            "entity unavailable on architecture"
        );
        Ok(row)
    }
    fn full_name(&self, row: &Declaration<'_>) -> String {
        match row {
            Declaration::Type(t) => self.names[&key(*t, 2)].clone(),
            Declaration::Reference(r) => {
                if r.namespace().is_empty() {
                    r.name().into()
                } else {
                    format!("{}.{}", r.namespace(), r.name())
                }
            }
            _ => format!(
                "{}.{}",
                self.names[&key(row.owner().unwrap(), 2)],
                row.name()
            ),
        }
    }
    /// Summarize an already resolved declaration and its ID from this Catalog.
    pub fn summarize(&self, id: EntityId, row: &Declaration<'_>) -> EntitySummary {
        let kind = match row {
            Declaration::Type(t) => {
                if t.name() == "<Module>" {
                    "module".into()
                } else {
                    format!("{:?}", t.category()).to_lowercase()
                }
            }
            Declaration::Field(f) => if f.constant().is_some() {
                "constant"
            } else {
                "field"
            }
            .into(),
            Declaration::Method(_) => if row.owner().unwrap().name() == "Apis" {
                "function"
            } else {
                "method"
            }
            .into(),
            Declaration::Property(_) => "property".into(),
            Declaration::Event(_) => "event".into(),
            Declaration::Reference(_) => "type_reference".into(),
        };
        EntitySummary {
            id,
            name: row.name().into(),
            full_name: self.full_name(row),
            kind,
            architecture_mask: row.architecture_mask(),
        }
    }
    pub fn entity(&self, id: EntityId) -> Result<EntitySummary> {
        Ok(self.summarize(id, &self.declaration(id, None)?))
    }
    pub fn resolve_reference(&self, id: EntityId, arch: Architecture) -> Result<EntityId> {
        let r: TypeRef = self.db.row(id, 1)?;
        let owner = self.db.row(
            EntityId {
                source_id: id.source_id,
                token: 0x02000001,
            },
            2,
        )?;
        Ok(key(
            self.db
                .resolve(&TypeDefOrRef::TypeRef(r).type_name(), owner, arch.bit())?,
            2,
        ))
    }
    pub fn layout(&self, id: EntityId, arch: Architecture) -> Result<Layout> {
        crate::layout::derive_key(&self.db, id, arch)
    }
    pub fn vtable(&self, id: EntityId, arch: Architecture) -> Result<Vtable> {
        self.db.vtable_key(id, arch)
    }
    pub fn search<'a>(&'a self, query: &str, options: SearchOptions<'a>) -> Result<Search<'a>> {
        ensure!(!query.is_empty(), "empty search query");
        let query = if options.case_sensitive {
            query.into()
        } else {
            query.to_lowercase()
        };
        let matcher = match options.mode {
            SearchMode::Integer => Matcher::Integer(if let Some(h) = query.strip_prefix("0x") {
                i128::from_str_radix(h, 16)?
            } else {
                query.parse()?
            }),
            SearchMode::Guid => {
                let g = query.trim_matches(['{', '}']).to_owned();
                ensure!(
                    g.len() == 36
                        && [8, 13, 18, 23].iter().all(|i| g.as_bytes()[*i] == b'-')
                        && g.bytes()
                            .enumerate()
                            .all(|(i, c)| [8, 13, 18, 23].contains(&i) || c.is_ascii_hexdigit()),
                    "invalid GUID"
                );
                Matcher::Guid(g)
            }
            _ => Matcher::Name(query),
        };
        self.iter(matcher, options)
    }
    pub fn references<'a>(
        &'a self,
        target: EntityId,
        arch: Architecture,
        cancel: Option<&'a AtomicBool>,
    ) -> Result<Search<'a>> {
        let t: TypeDef = self.db.row(target, 2)?;
        ensure!(supports(t, arch.bit()), "type unavailable on architecture");
        self.iter(
            Matcher::References(target, arch),
            SearchOptions {
                architecture: Some(arch),
                cancel,
                ..Default::default()
            },
        )
    }
    fn iter<'a>(&'a self, matcher: Matcher, options: SearchOptions<'a>) -> Result<Search<'a>> {
        ensure!(
            options.source.is_none_or(|i| i < self.sources().len()),
            "source index out of range"
        );
        Ok(Search {
            catalog: self,
            options,
            matcher,
            position: 0,
            done: false,
            unresolved: 0,
        })
    }
}
enum Matcher {
    Name(String),
    Integer(i128),
    Guid(String),
    References(EntityId, Architecture),
}
/// Lazy, source/token-ordered results. Errors terminate iteration; dropping stops the query.
pub struct Search<'a> {
    catalog: &'a Catalog,
    options: SearchOptions<'a>,
    matcher: Matcher,
    position: usize,
    done: bool,
    unresolved: usize,
}
impl Search<'_> {
    /// Unresolved type uses encountered so far by a reference query.
    pub fn unresolved_type_uses(&self) -> usize {
        self.unresolved
    }
    fn scan(&mut self) -> Result<Option<EntitySummary>> {
        while let Some(&id) = self.catalog.search_index.get(self.position) {
            ensure!(
                !self
                    .options
                    .cancel
                    .is_some_and(|c| c.load(Ordering::Relaxed)),
                "query cancelled"
            );
            self.position += 1;
            if self.options.source.is_some_and(|s| s != id.source_id)
                || self.options.types_only && id.token >> 24 != 2
            {
                continue;
            }
            let row = self.catalog.declaration(id, None)?;
            if self
                .options
                .architecture
                .is_some_and(|a| row.architecture_mask() & a.bit() == 0)
            {
                continue;
            }
            let matches = match &self.matcher {
                Matcher::Name(query) => {
                    let full = self.catalog.full_name(&row);
                    [row.name(), &full].iter().any(|name| {
                        let normalized;
                        let name = if self.options.case_sensitive {
                            *name
                        } else {
                            normalized = name.to_lowercase();
                            &normalized
                        };
                        match self.options.mode {
                            SearchMode::Exact => name == query,
                            SearchMode::Prefix => name.starts_with(query),
                            _ => name.contains(query),
                        }
                    })
                }
                Matcher::Integer(n) => {
                    matches!(&row,Declaration::Field(f) if f.constant().and_then(|c|integer(&c.value()))==Some(*n))
                }
                Matcher::Guid(g) => {
                    match &row {
                        Declaration::Type(t) => guid(*t),
                        Declaration::Field(f) => guid(*f),
                        _ => None,
                    }
                    .as_ref()
                        == Some(g)
                }
                Matcher::References(target, arch) => {
                    let owner = row.owner().unwrap();
                    let mut check = |ty: &Type| {
                        uses(
                            &self.catalog.db,
                            ty,
                            owner,
                            arch.bit(),
                            *target,
                            &mut self.unresolved,
                        )
                    };
                    match &row {
                        Declaration::Type(t) => t
                            .interface_impls()
                            .any(|i| check(&i.interface(&t.generic_types()))),
                        Declaration::Field(f) => check(&f.ty()),
                        Declaration::Event(e) => check(&e.ty()),
                        Declaration::Method(m) => {
                            let s = m.signature(&[]);
                            s.types
                                .iter()
                                .fold(check(&s.return_type), |hit, t| check(t) || hit)
                        }
                        Declaration::Property(p) => {
                            let s = p.signature();
                            s.types
                                .iter()
                                .fold(check(&s.return_type), |hit, t| check(t) || hit)
                        }
                        Declaration::Reference(_) => false,
                    }
                }
            };
            if matches {
                return Ok(Some(self.catalog.summarize(id, &row)));
            }
        }
        ensure!(
            !self
                .options
                .cancel
                .is_some_and(|c| c.load(Ordering::Relaxed)),
            "query cancelled"
        );
        Ok(None)
    }
}
impl Iterator for Search<'_> {
    type Item = Result<EntitySummary>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        match self.scan() {
            Ok(Some(item)) => Some(Ok(item)),
            Ok(None) => {
                self.done = true;
                None
            }
            Err(e) => {
                self.done = true;
                Some(Err(e))
            }
        }
    }
}
impl std::iter::FusedIterator for Search<'_> {}
fn uses(
    db: &Db,
    t: &Type,
    owner: TypeDef<'_>,
    arch: i32,
    target: EntityId,
    unresolved: &mut usize,
) -> bool {
    match t {
        Type::ClassName(n) | Type::ValueName(n) => {
            let mut definition = n.clone();
            definition.generics.clear();
            let direct = match db.resolve(&definition, owner, arch) {
                Ok(t) => key(t, 2) == target,
                Err(_) => {
                    *unresolved += 1;
                    false
                }
            };
            n.generics.iter().fold(direct, |hit, t| {
                uses(db, t, owner, arch, target, unresolved) || hit
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

#[cfg(test)]
mod tests {
    use super::Architecture;

    #[test]
    fn architecture_masks_are_domain_values() {
        assert_eq!(Architecture::ALL_MASK, 7);
        assert!(Architecture::X86.supports(1));
        assert!(Architecture::X64.supports(2));
        assert!(Architecture::Arm64.supports(4));
        assert!(!Architecture::X86.supports(2));
    }

    #[test]
    fn architecture_names_parse_back() {
        for architecture in [Architecture::X86, Architecture::X64, Architecture::Arm64] {
            assert_eq!(
                architecture.name().parse::<Architecture>().unwrap(),
                architecture
            );
        }
        assert!("ARM64".parse::<Architecture>().is_err());
    }
}
