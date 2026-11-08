use crate::{
    metadata::{Db, key, mask, supports},
    model::*,
    search::{MatchField, MatchQuality, SearchMatch, SearchQuery},
    source::{EntityId, Source},
};
use anyhow::{Context, Result, bail, ensure};
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
    /// Canonical presentation and fallback order, independent of user preference.
    pub const ALL: [Self; 3] = [Self::X86, Self::X64, Self::Arm64];
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
    /// Canonical order of the architectures selected by a mask.
    pub fn supported(mask: i32) -> impl Iterator<Item = Self> {
        Self::ALL
            .into_iter()
            .filter(move |arch| arch.supports(mask))
    }
    pub const fn pointer_bytes(self) -> usize {
        match self {
            Self::X86 => 4,
            Self::X64 | Self::Arm64 => 8,
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
        Self::ALL
            .into_iter()
            .find(|arch| arch.name() == s)
            .ok_or_else(|| anyhow::anyhow!("architecture must be x86, x64 or arm64"))
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SearchMode {
    #[default]
    Combined,
    Name,
    Value,
    Guid,
    Condition,
}
#[derive(Debug)]
pub struct InvalidSearchInput(pub SearchMode);
impl std::fmt::Display for InvalidSearchInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "invalid {:?} search input", self.0)
    }
}
impl std::error::Error for InvalidSearchInput {}
#[derive(Default)]
pub struct SearchOptions<'a> {
    pub source: Option<usize>,
    pub case_sensitive: bool,
    pub mode: SearchMode,
    pub architecture: Option<Architecture>,
    pub categories: Option<u32>,
    pub cancel: Option<&'a AtomicBool>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum EntityCategory {
    Type = 1,
    Method = 2,
    Constant = 4,
    Field = 8,
    Property = 16,
    Event = 32,
    TypeReference = 64,
}
impl EntityCategory {
    pub const ALL: u32 = 127;
    pub const DEFAULT: u32 = Self::Type as u32 | Self::Method as u32 | Self::Constant as u32;

    pub(crate) const NAMES: &[(Self, &str)] = &[
        (Self::Type, "type"),
        (Self::Method, "method"),
        (Self::Constant, "constant"),
        (Self::Field, "field"),
        (Self::Property, "property"),
        (Self::Event, "event"),
        (Self::TypeReference, "type_reference"),
    ];

    fn name(self) -> &'static str {
        Self::NAMES
            .iter()
            .find(|(category, _)| *category == self)
            .unwrap()
            .1
    }
}
#[derive(Debug, serde::Serialize)]
pub struct EntitySummary {
    pub id: EntityId,
    pub name: String,
    pub full_name: String,
    pub kind: String,
    pub architecture_mask: i32,
}
pub struct EntityContext {
    pub namespace: String,
    pub owner: Option<String>,
    pub constant: Option<Value>,
    pub guid: Option<String>,
}
struct TypeName {
    full: String,
    namespace_len: usize,
}
pub struct Catalog {
    db: Db,
    names: HashMap<EntityId, TypeName>,
    search_index: Vec<EntityId>,
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
    pub(crate) fn constant_value(&self) -> Option<Value> {
        match self {
            Self::Field(field) => field.constant().map(|value| value.value()),
            _ => None,
        }
    }
    pub(crate) fn guid(&self) -> Option<String> {
        match self {
            Self::Type(row) => guid(*row),
            Self::Field(row) => guid(*row),
            _ => None,
        }
    }
    pub fn documentation_url(&self) -> Option<String> {
        fn read<'a>(row: impl HasAttributes<'a>) -> Option<String> {
            row.find_attribute("DocumentationAttribute")?
                .value()
                .into_iter()
                .find_map(|(_, value)| match value {
                    Value::Utf8(url) => Some(url.to_string()),
                    _ => None,
                })
        }
        match self {
            Self::Type(row) => read(*row),
            Self::Method(row) => read(*row),
            Self::Field(row) => read(*row),
            Self::Property(row) => read(*row),
            Self::Event(row) => read(*row),
            Self::Reference(_) => None,
        }
    }
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
            Self::Type(r) => return mask(*r),
            Self::Field(r) => mask(*r),
            Self::Method(r) => mask(*r),
            Self::Property(r) => mask(*r),
            Self::Event(r) => mask(*r),
            Self::Reference(_) => Architecture::ALL_MASK,
        };
        member & self.owner().map(mask).unwrap_or(Architecture::ALL_MASK)
    }
    pub fn category(&self) -> EntityCategory {
        match self {
            Self::Type(_) => EntityCategory::Type,
            Self::Method(_) => EntityCategory::Method,
            Self::Field(f) if f.constant().is_some() => EntityCategory::Constant,
            Self::Field(_) => EntityCategory::Field,
            Self::Property(_) => EntityCategory::Property,
            Self::Event(_) => EntityCategory::Event,
            Self::Reference(_) => EntityCategory::TypeReference,
        }
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
        ) -> Result<TypeName> {
            ensure!(depth < 128, "nested type depth exceeds limit");
            let file = t.to_row().file;
            if let Some(parent) = parents.get(&(file, t.pos())) {
                let p = db.row::<TypeDef>(
                    EntityId {
                        source_id: file,
                        token: 0x02000000 | (*parent as u32 + 1),
                    },
                    2,
                )?;
                let parent = full(db, p, parents, depth + 1)?;
                Ok(TypeName {
                    full: format!("{}+{}", parent.full, t.name()),
                    namespace_len: parent.namespace_len,
                })
            } else {
                Ok(TypeName {
                    full: if t.namespace().is_empty() {
                        t.name().into()
                    } else {
                        format!("{}.{}", t.namespace(), t.name())
                    },
                    namespace_len: t.namespace().len(),
                })
            }
        }
        let mut names = HashMap::new();
        let mut search_index = Vec::new();
        for file in 0..db.sources.len() {
            search_index.extend(db.index.rows::<TypeRef>(file).map(|r| key(r, 1)));
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
            Declaration::Type(t) => self.names[&key(*t, 2)].full.clone(),
            Declaration::Reference(r) => {
                if r.namespace().is_empty() {
                    r.name().into()
                } else {
                    format!("{}.{}", r.namespace(), r.name())
                }
            }
            _ => format!(
                "{}.{}",
                self.names[&key(row.owner().unwrap(), 2)].full,
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
            _ => row.category().name().into(),
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
    /// Metadata-owned context, without interpreting container names or member flags.
    pub fn entity_context(&self, id: EntityId) -> Result<EntityContext> {
        let row = self.declaration(id, None)?;
        let (namespace, owner) = self.name_context(&row);
        Ok(EntityContext {
            namespace: namespace.to_owned(),
            owner: owner.map(str::to_owned),
            constant: row.constant_value(),
            guid: row.guid(),
        })
    }
    fn name_context<'a>(&'a self, row: &Declaration<'a>) -> (&'a str, Option<&'a str>) {
        if let Some(owner) = row.owner() {
            let name = &self.names[&key(owner, 2)];
            (
                &name.full[..name.namespace_len],
                (!matches!(row, Declaration::Type(_))).then(|| {
                    &name.full[name.namespace_len + usize::from(name.namespace_len != 0)..]
                }),
            )
        } else if let Declaration::Reference(row) = row {
            (row.namespace(), None)
        } else {
            ("", None)
        }
    }
    /// Resolve a named type's definition in the declaration owner's context.
    pub fn resolve_type(
        &self,
        owner: EntityId,
        name: &windows_metadata::TypeName,
        arch: Architecture,
    ) -> Result<EntityId> {
        let declaration = self.declaration(owner, Some(arch))?;
        let context = match declaration.owner() {
            Some(context) => context,
            None => self.db.row(
                EntityId {
                    source_id: owner.source_id,
                    token: 0x02000001,
                },
                2,
            )?,
        };
        let mut definition = name.clone();
        definition.generics.clear();
        Ok(key(self.db.resolve(&definition, context, arch.bit())?, 2))
    }

    pub fn resolve_reference(&self, id: EntityId, arch: Architecture) -> Result<EntityId> {
        let r: TypeRef = self.db.row(id, 1)?;
        self.resolve_type(id, &TypeDefOrRef::TypeRef(r).type_name(), arch)
    }
    pub fn layout(&self, id: EntityId, arch: Architecture) -> Result<Layout> {
        crate::layout::derive_key(&self.db, id, arch)
    }
    pub fn vtable(&self, id: EntityId, arch: Architecture) -> Result<Vtable> {
        self.db.vtable_key(id, arch)
    }
    pub fn search<'a>(&'a self, query: &str, options: SearchOptions<'a>) -> Result<Search<'a>> {
        ensure!(!query.is_empty(), "empty search query");
        self.search_compiled(
            SearchQuery::compile(query, options.mode, options.case_sensitive)
                .with_context(|| InvalidSearchInput(options.mode))?,
            options,
        )
    }
    pub fn search_compiled<'a>(
        &'a self,
        query: crate::CompiledQuery,
        options: SearchOptions<'a>,
    ) -> Result<Search<'a>> {
        self.iter(Matcher::Query(Box::new(query)), options)
    }
    /// Exact short or full name lookup, without scope browsing or combined search.
    pub fn search_exact<'a>(
        &'a self,
        query: &str,
        options: SearchOptions<'a>,
    ) -> Result<Search<'a>> {
        ensure!(!query.is_empty(), "empty search query");
        self.iter(
            Matcher::Exact(if options.case_sensitive {
                query.to_owned()
            } else {
                query.to_lowercase()
            }),
            options,
        )
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
            options
                .categories
                .is_none_or(|mask| mask & !EntityCategory::ALL == 0),
            "invalid search category mask"
        );
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
    Exact(String),
    Query(Box<SearchQuery>),
    References(EntityId, Architecture),
}
/// Lazy scan order, not relevance order. Errors terminate iteration; dropping stops the query.
pub struct Search<'a> {
    catalog: &'a Catalog,
    options: SearchOptions<'a>,
    matcher: Matcher,
    position: usize,
    done: bool,
    unresolved: usize,
}
#[derive(Debug)]
pub enum SearchStep {
    Match(SearchMatch),
    Skipped,
    Done,
}
impl Search<'_> {
    /// Unresolved type uses encountered so far by a reference query.
    pub fn unresolved_type_uses(&self) -> usize {
        self.unresolved
    }
    /// Scan at most one candidate. Completion and errors fuse both this method and the iterator.
    pub fn step(&mut self) -> Result<SearchStep> {
        if self.done {
            return Ok(SearchStep::Done);
        }
        let result = self.scan_one();
        if matches!(&result, Ok(SearchStep::Done) | Err(_)) {
            self.done = true;
        }
        result
    }
    fn scan_one(&mut self) -> Result<SearchStep> {
        ensure!(
            !self
                .options
                .cancel
                .is_some_and(|c| c.load(Ordering::Relaxed)),
            "query cancelled"
        );
        if let Some(&id) = self.catalog.search_index.get(self.position) {
            self.position += 1;
            if self.options.source.is_some_and(|s| s != id.source_id) {
                return Ok(SearchStep::Skipped);
            }
            let row = self.catalog.declaration(id, None)?;
            if self
                .options
                .categories
                .is_some_and(|mask| mask & row.category() as u32 == 0)
            {
                return Ok(SearchStep::Skipped);
            }
            if self
                .options
                .architecture
                .is_some_and(|a| row.architecture_mask() & a.bit() == 0)
            {
                return Ok(SearchStep::Skipped);
            }
            let mut evidence = Vec::new();
            let quality = match &mut self.matcher {
                Matcher::Exact(query) => {
                    let full = self.catalog.full_name(&row);
                    let equal = |text: &str| {
                        if self.options.case_sensitive {
                            text == query
                        } else {
                            text.to_lowercase() == *query
                        }
                    };
                    if equal(row.name()) {
                        Some(MatchQuality::NameExact(MatchField::Name))
                    } else if equal(&full) {
                        Some(MatchQuality::NameExact(MatchField::FullName))
                    } else {
                        None
                    }
                }
                Matcher::Query(query) => {
                    let full = if query.context_needed {
                        self.catalog.full_name(&row)
                    } else {
                        String::new()
                    };
                    let namespace = if query.context_needed {
                        self.catalog.name_context(&row).0
                    } else {
                        ""
                    };
                    query
                        .matches(
                            &row,
                            &full,
                            namespace,
                            id.source_id,
                            self.options.architecture,
                        )
                        .map(|(quality, found)| {
                            evidence = found;
                            quality
                        })
                }
                Matcher::References(target, arch) => {
                    let Some(owner) = row.owner() else {
                        return Ok(SearchStep::Skipped);
                    };
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
                    fn signature_uses(s: Signature, mut check: impl FnMut(&Type) -> bool) -> bool {
                        s.types
                            .iter()
                            .fold(check(&s.return_type), |hit, t| check(t) || hit)
                    }
                    let matches = match &row {
                        Declaration::Type(t) => {
                            t.interface_impls().fold(false, |hit, interface| {
                                hit | check(&interface.interface(&t.generic_types()))
                            })
                        }
                        Declaration::Field(f) => check(&f.ty()),
                        Declaration::Event(e) => check(&e.ty()),
                        Declaration::Method(m) => signature_uses(m.signature(&[]), &mut check),
                        Declaration::Property(p) => signature_uses(p.signature(), &mut check),
                        Declaration::Reference(_) => false,
                    };
                    matches.then_some(MatchQuality::Reference)
                }
            };
            if let Some(quality) = quality {
                return Ok(SearchStep::Match(SearchMatch {
                    entity: self.catalog.summarize(id, &row),
                    quality,
                    evidence,
                }));
            }
            return Ok(SearchStep::Skipped);
        }
        Ok(SearchStep::Done)
    }
}
impl Iterator for Search<'_> {
    type Item = Result<SearchMatch>;
    fn next(&mut self) -> Option<Self::Item> {
        loop {
            match self.step() {
                Ok(SearchStep::Match(item)) => return Some(Ok(item)),
                Ok(SearchStep::Skipped) => {}
                Ok(SearchStep::Done) => return None,
                Err(e) => return Some(Err(e)),
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
    use crate::test_support::{FixtureFile, source as data};

    #[test]
    fn named_resolution_preserves_dependencies_architecture_and_snapshots() {
        use crate::{Catalog, Declaration};
        use windows_metadata::{reader, writer, *};
        let dependency = |field_type: Type| {
            let mut file = writer::File::new("NavigationDependency");
            let ty = file.TypeDef(
                "Fixture",
                "Widget",
                writer::TypeDefOrRef::default(),
                TypeAttributes::Public | TypeAttributes::WindowsRuntime,
            );
            file.Field("Value", &field_type, FieldAttributes::Public);
            let ns = "Windows.Win32.Foundation.Metadata";
            let attr = file.TypeRef(ns, "SupportedArchitectureAttribute");
            let ctor = file.constructor(attr, vec![Type::value_named(ns, "Architecture")]);
            file.Attribute(
                writer::HasAttribute::TypeDef(ty),
                writer::AttributeType::MemberRef(ctor),
                &[(
                    String::new(),
                    Value::EnumValue(TypeName::named(ns, "Architecture"), Box::new(Value::I32(2))),
                )],
            );
            data("NavigationDependency", file)
        };
        let original = dependency(Type::I32);
        let mut file = writer::File::new("NavigationConsumer");
        file.set_reference(reader::Index::new(vec![
            reader::File::new_shared(original.bytes.clone()).unwrap(),
        ]));
        file.TypeDef(
            "Fixture",
            "Owner",
            writer::TypeDefOrRef::default(),
            TypeAttributes::Public,
        );
        file.Field(
            "Member",
            &Type::value_named("Fixture", "Widget"),
            FieldAttributes::Public,
        );
        let consumer = data("NavigationConsumer", file);
        let missing = Catalog::from_inputs([consumer.clone()]).unwrap();
        let old = Catalog::from_inputs([consumer.clone(), original.clone()]).unwrap();
        let current = Catalog::from_inputs([consumer.clone(), dependency(Type::I64)]).unwrap();
        let ambiguous = Catalog::from_inputs([consumer, original.clone(), original]).unwrap();
        let member = |catalog: &Catalog| {
            crate::test_support::first_match(catalog.search(
                "Member",
                crate::SearchOptions {
                    mode: crate::SearchMode::Name,
                    ..Default::default()
                },
            ))
            .id
        };
        let field_name = |catalog: &Catalog| {
            let Declaration::Field(field) = catalog.declaration(member(catalog), None).unwrap()
            else {
                panic!()
            };
            let Type::ValueName(mut name) = field.ty() else {
                panic!()
            };
            name.generics = vec![Type::I32];
            name
        };
        for catalog in [&missing, &ambiguous] {
            assert!(
                catalog
                    .resolve_type(member(catalog), &field_name(catalog), Architecture::X64)
                    .is_err()
            );
        }
        for (catalog, expected) in [(&old, Type::I32), (&current, Type::I64)] {
            let target = catalog
                .resolve_type(member(catalog), &field_name(catalog), Architecture::X64)
                .unwrap();
            assert_eq!(target.source_id, 1);
            assert_eq!(catalog.entity(target).unwrap().full_name, "Fixture.Widget");
            assert!(
                catalog
                    .resolve_type(member(catalog), &field_name(catalog), Architecture::X86)
                    .is_err()
            );
            let Declaration::Type(ty) = catalog.declaration(target, None).unwrap() else {
                panic!()
            };
            let value = ty.fields().next().unwrap().ty();
            assert_eq!(value, expected);
        }
        assert!(
            missing
                .resolve_type(member(&missing), &field_name(&missing), Architecture::X64)
                .is_err()
        );
        let target = old
            .resolve_type(member(&old), &field_name(&old), Architecture::X64)
            .unwrap();
        assert!(
            old.resolve_type(target, &field_name(&old), Architecture::X86)
                .is_err()
        );
    }

    #[test]
    fn canonical_architectures_preserve_names_bits_widths_and_strict_parsing() {
        let expected = [
            (Architecture::X86, "x86", 1, 4),
            (Architecture::X64, "x64", 2, 8),
            (Architecture::Arm64, "arm64", 4, 8),
        ];
        assert_eq!(Architecture::ALL, expected.map(|(arch, _, _, _)| arch));
        assert_eq!(Architecture::ALL_MASK, 7);
        for (mask, names) in [
            (0, vec![]),
            (1, vec!["x86"]),
            (2, vec!["x64"]),
            (4, vec!["arm64"]),
            (5, vec!["x86", "arm64"]),
            (7, vec!["x86", "x64", "arm64"]),
        ] {
            assert_eq!(
                Architecture::supported(mask)
                    .map(Architecture::name)
                    .collect::<Vec<_>>(),
                names
            );
        }
        for (arch, name, bit, width) in expected {
            assert_eq!(arch.name(), name);
            assert_eq!(name.parse::<Architecture>().unwrap(), arch);
            assert_eq!(arch.bit(), bit);
            assert_eq!(arch.pointer_bytes(), width);
            for mask in -1..=15 {
                assert_eq!(arch.supports(mask), mask & bit != 0);
            }
        }
        for text in [
            "", "X86", "X64", "ARM64", " x86", "x86 ", "amd64", "x86\0", "窗口",
        ] {
            assert_eq!(
                text.parse::<Architecture>().unwrap_err().to_string(),
                "architecture must be x86, x64 or arm64"
            );
        }
    }

    #[test]
    fn documentation_links_are_read_from_each_declaration() {
        use windows_metadata::{reader::*, writer, *};
        let mut file = writer::File::new("DocumentationFixture");
        let object = file.TypeRef("System", "Object");
        let attribute = file.TypeRef(
            "Windows.Win32.Foundation.Metadata",
            "DocumentationAttribute",
        );
        let constructor = file.constructor(attribute, vec![Type::String]);
        let ty = file.public_type("Fixture", "Documented", object);
        let field = file.Field("Value", &Type::I32, FieldAttributes::Public);
        let method = file.MethodDef(
            "Call",
            &Signature {
                flags: MethodCallAttributes::default(),
                return_type: Type::Void,
                types: vec![],
            },
            MethodAttributes::Public,
            MethodImplAttributes::default(),
        );
        for (owner, url) in [
            (
                writer::HasAttribute::TypeDef(ty),
                "https://example.com/type",
            ),
            (
                writer::HasAttribute::Field(field),
                "https://example.com/field",
            ),
            (
                writer::HasAttribute::MethodDef(method),
                "https://example.com/method",
            ),
        ] {
            file.Attribute(
                owner,
                writer::AttributeType::MemberRef(constructor),
                &[(String::new(), Value::Utf8(url.into()))],
            );
        }
        file.public_type("Fixture", "Undocumented", object);
        let index = Index::new(vec![File::new(file.into_stream()).unwrap()]);
        let find_type = |name| {
            index
                .rows::<TypeDef>(0)
                .find(|ty| ty.namespace() == "Fixture" && ty.name() == name)
                .unwrap()
        };
        assert_eq!(
            super::Declaration::Type(find_type("Documented"))
                .documentation_url()
                .as_deref(),
            Some("https://example.com/type")
        );
        assert_eq!(
            super::Declaration::Type(find_type("Undocumented")).documentation_url(),
            None
        );
        assert_eq!(
            super::Declaration::Field(index.rows::<Field>(0).next().unwrap())
                .documentation_url()
                .as_deref(),
            Some("https://example.com/field")
        );
        assert_eq!(
            super::Declaration::Method(index.rows::<MethodDef>(0).next().unwrap())
                .documentation_url()
                .as_deref(),
            Some("https://example.com/method")
        );
    }
}
