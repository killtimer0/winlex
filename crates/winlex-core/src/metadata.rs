use crate::source::{AssemblyIdentity, Key, Source};
use anyhow::{Result, bail, ensure};
use serde_json::{Value as J, json};
use std::{
    collections::{BTreeMap, HashSet},
    path::Path,
};
use windows_metadata::{reader::*, *};

pub struct Db {
    pub inputs: Vec<crate::SourceData>,
    pub index: Index,
    pub type_count: usize,
    pub field_count: usize,
    pub method_count: usize,
    pub sources: Vec<Source>,
    types: Vec<(usize, usize)>,
}

pub fn token<'a, R: AsRow<'a>>(r: R, table: u32) -> u32 {
    table << 24 | (r.pos() + 1) as u32
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
        assert_eq!(self.sources.len(), 1, "reader-facts requires one source");
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
        let properties: Vec<_> = self.index.rows::<Property>(0).map(|p|json!({"token":token(p,0x17),"name":p.name(),"parent":token(p.parent(),2),"accessors":p.accessors().map(|s|json!({"flags":s.flags(),"method":token(s.method(),6)})).collect::<Vec<_>>()})).collect();
        let events: Vec<_> = self.index.rows::<Event>(0).map(|e|json!({"token":token(e,0x14),"name":e.name(),"parent":token(e.parent(),2),"accessors":e.accessors().map(|s|json!({"flags":s.flags(),"method":token(s.method(),6)})).collect::<Vec<_>>()})).collect();
        json!({"assembly":self.sources[0].assembly,"references":self.sources[0].references,"layouts":layouts,"scopes":scopes,"signature_type_tokens":signatures,"properties":properties,"events":events})
    }
    pub fn open(path: &Path) -> Result<Self> {
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
    pub fn from_inputs(data: impl IntoIterator<Item = crate::SourceData>) -> Result<Self> {
        let mut inputs = BTreeMap::new();
        for source in data {
            inputs.entry(source.id.clone()).or_insert(source);
        }
        ensure!(!inputs.is_empty(), "at least one WinMD is required");
        let mut files = vec![];
        let mut sources = vec![];
        let inputs: Vec<_> = inputs.into_values().collect();
        for input in &inputs {
            let path = &input.path;
            files.push(
                File::new_shared(input.bytes.clone())
                    .ok_or_else(|| anyhow::anyhow!("invalid metadata: {}", path.display()))?,
            );
            sources.push(Source {
                id: input.id.clone(),
                path: path.display().to_string(),
                native_path: path.clone(),
                assembly: None,
                references: vec![],
            });
        }
        let index = Index::new(files);
        let mut types = vec![];
        let (mut field_count, mut method_count) = (0, 0);
        for (file, source) in sources.iter_mut().enumerate() {
            let mut assemblies = index.rows::<Assembly>(file);
            source.assembly = assemblies.next().map(AssemblyIdentity::definition);
            ensure!(
                assemblies.next().is_none(),
                "multiple Assembly rows in {}",
                source.path
            );
            source.references = index
                .rows::<AssemblyRef>(file)
                .map(AssemblyIdentity::reference)
                .collect();
            types.extend(index.rows::<TypeDef>(file).map(|r| (file, r.pos())));
            field_count += index.rows::<Field>(file).len();
            method_count += index.rows::<MethodDef>(file).len();
        }
        Ok(Self {
            inputs,
            index,
            type_count: types.len(),
            field_count,
            method_count,
            sources,
            types,
        })
    }
    pub(crate) fn source_file(&self, selector: &str) -> Result<usize> {
        let found: Vec<_> = self
            .sources
            .iter()
            .enumerate()
            .filter(|(_, s)| {
                s.id == selector || s.assembly.as_ref().is_some_and(|a| a.name == selector)
            })
            .map(|(i, _)| i)
            .collect();
        ensure!(
            found.len() == 1,
            "source selector {selector:?} matches {} files; use a source SHA256",
            found.len()
        );
        Ok(found[0])
    }
    pub(crate) fn id(&self, key: Key) -> String {
        format!("{}::0x{:08x}", self.sources[key.file].id, key.token)
    }
    pub(crate) fn key<'a, R: AsRow<'a>>(&self, row: R, table: u32) -> Key {
        Key {
            file: row.to_row().file,
            token: token(row, table),
        }
    }
    pub(crate) fn row<'a, R: AsRow<'a> + 'a>(&'a self, key: Key, table: u32) -> Result<R> {
        ensure!(
            key.file < self.sources.len() && key.token >> 24 == table,
            "invalid source or token table"
        );
        let pos = (key.token & 0xffffff)
            .checked_sub(1)
            .ok_or_else(|| anyhow::anyhow!("nil token"))? as usize;
        ensure!(
            pos < self.index.rows::<R>(key.file).len(),
            "out-of-range token {}",
            self.id(key)
        );
        Ok(R::from_row(Row {
            index: &self.index,
            file: key.file,
            pos,
        }))
    }
    pub fn ty(&self, pos: usize) -> TypeDef<'_> {
        TypeDef::from_row(Row {
            index: &self.index,
            file: self.types[pos].0,
            pos: self.types[pos].1,
        })
    }
    pub fn resolve(&self, n: &TypeName, owner: TypeDef<'_>, arch: i32) -> Result<TypeDef<'_>> {
        self.resolve_inner(n, owner, arch, 0)
    }
    fn resolve_inner(
        &self,
        n: &TypeName,
        owner: TypeDef<'_>,
        arch: i32,
        depth: usize,
    ) -> Result<TypeDef<'_>> {
        ensure!(depth < 128, "TypeRef scope cycle/depth exceeded");
        ensure!(n.generics.is_empty(), "generic type unsupported");
        let mut target_file = owner.to_row().file;
        if let Some((file, tok)) = n.identity {
            target_file = file;
            if tok >> 24 == 2 {
                let t: TypeDef = self.row(Key { file, token: tok }, 2)?;
                ensure!(
                    supports(t, arch),
                    "TypeDef reference unavailable on architecture"
                );
                return Ok(t);
            }
            ensure!(tok >> 24 == 1, "unhandled signature reference token");
            let r: TypeRef = self.row(Key { file, token: tok }, 1)?;
            let scope = r.scope_token();
            if scope >> 24 == 1 {
                let parent: TypeRef = self.row(Key { file, token: scope }, 1)?;
                let mut pn = TypeName::named(parent.namespace(), parent.name());
                pn.identity = Some((file, scope));
                let p = self.resolve_inner(&pn, owner, arch, depth + 1)?;
                let found: Vec<_> = self
                    .index
                    .nested(p)
                    .filter(|t| t.name() == n.name && supports(*t, arch))
                    .collect();
                ensure!(found.len() == 1, "ambiguous nested reference");
                return Ok(found[0]);
            }
            if scope >> 24 == 0x23 {
                let reference: AssemblyRef = self.row(Key { file, token: scope }, 0x23)?;
                ensure!(
                    reference.flags() & 0x100 == 0,
                    "retargetable AssemblyRef unsupported"
                );
                let identity = AssemblyIdentity::reference(reference);
                if identity.content_type == 0x200 {
                    // WinRT identity is the fully qualified type name, not CLR assembly
                    // version identity. Only WinRT definitions participate. Multiple
                    // loaded metadata sets remain ambiguous rather than first-wins.
                    let found: Vec<_> = self
                        .index
                        .get(&n.namespace, n.name.split('`').next().unwrap())
                        .filter(|t| {
                            t.name() == n.name
                                && t.flags().contains(TypeAttributes::WindowsRuntime)
                                && supports(*t, arch)
                                && self.sources[t.to_row().file]
                                    .assembly
                                    .as_ref()
                                    .is_some_and(|a| a.content_type == 0x200)
                        })
                        .collect();
                    ensure!(
                        found.len() == 1,
                        "WinRT type {}.{} has {} matching definitions; load its metadata dependency or remove overlapping sources",
                        n.namespace,
                        n.name,
                        found.len()
                    );
                    return Ok(found[0]);
                }
                let found: Vec<_> = self
                    .sources
                    .iter()
                    .enumerate()
                    .filter(|(_, s)| s.assembly.as_ref().is_some_and(|a| a.matches(&identity)))
                    .map(|(i, _)| i)
                    .collect();
                ensure!(
                    found.len() == 1,
                    "external ResolutionScope: AssemblyRef {:?} has {} matching sources (missing dependency, identity mismatch or ambiguous assembly)",
                    identity,
                    found.len()
                );
                target_file = found[0];
            } else {
                ensure!(
                    scope == 1,
                    "unsupported ResolutionScope 0x{scope:08x} (ModuleRef/exported types are not resolved)"
                );
            }
        }
        let found: Vec<_> = if n.identity.is_none() && n.namespace.is_empty() {
            self.index
                .nested(owner)
                .filter(|t| t.name() == n.name && supports(*t, arch))
                .collect()
        } else if n.namespace.is_empty() {
            let nested: HashSet<_> = self
                .index
                .rows::<NestedClass>(target_file)
                .map(|r| r.usize(0) - 1)
                .collect();
            self.index
                .rows::<TypeDef>(target_file)
                .filter(|t| {
                    t.namespace().is_empty()
                        && t.name() == n.name
                        && !nested.contains(&t.pos())
                        && supports(*t, arch)
                })
                .collect()
        } else {
            self.index
                .get(&n.namespace, n.name.split('`').next().unwrap())
                .filter(|t| {
                    t.name() == n.name && t.to_row().file == target_file && supports(*t, arch)
                })
                .collect()
        };
        ensure!(found.len() == 1, "unresolved/ambiguous type {n:?}");
        Ok(found[0])
    }
    pub(crate) fn vtable_key(&self, key: Key, arch: &str) -> Result<J> {
        let bit = arch_bit(arch)?;
        let t: TypeDef = self.row(key, 2)?;
        ensure!(supports(t, bit), "interface unavailable on architecture");
        let name = t.name();
        let mut slots = Vec::new();
        if t.flags().contains(TypeAttributes::WindowsRuntime) {
            ensure!(
                t.generic_params().next().is_none(),
                "open generic WinRT vtable ABI is not supported"
            );
            let delegate = t.category() == TypeCategory::Delegate;
            ensure!(
                delegate || t.category() == TypeCategory::Interface,
                "select a WinRT interface or delegate, not a runtime class"
            );
            let unknown = ["QueryInterface", "AddRef", "Release"];
            let inspectable = ["GetIids", "GetRuntimeClassName", "GetTrustLevel"];
            for (declaring_type, methods) in [
                ("IUnknown", unknown.as_slice()),
                (
                    "IInspectable",
                    if delegate {
                        &[]
                    } else {
                        inspectable.as_slice()
                    },
                ),
            ] {
                for method in methods {
                    slots.push(json!({"index":slots.len(),"method":method,"declaring_type":declaring_type,"implicit":true,"id":null,"source_id":null,"token":null,"declaring_type_id":null}));
                }
            }
            // WinRT InterfaceImpl entries are requirements, not vtable inheritance.
            for m in t
                .methods()
                .filter(|m| supports(*m, bit) && (!delegate || m.name() == "Invoke"))
            {
                ensure!(
                    !m.flags().contains(MethodAttributes::Static),
                    "static interface method unsupported"
                );
                ensure!(
                    m.generic_params().next().is_none(),
                    "generic method ABI unsupported"
                );
                slots.push(json!({"index":slots.len(),"method":m.name(),"declaring_type":t.name(),"implicit":false,"token":format!("0x{:08x}",token(m,6)),"source_id":self.sources[key.file].id,"id":self.id(self.key(m,6)),"declaring_type_id":self.id(key)}));
            }
            ensure!(
                !delegate || slots.len() == 4,
                "WinRT delegate must have one Invoke method"
            );
            return Ok(
                json!({"name":name,"arch":arch,"id":self.id(key),"source_id":self.sources[key.file].id,"abi":"winrt","scope":"method slots only; metadata signatures are not lowered ABI signatures","slots":slots}),
            );
        }
        let mut active = HashSet::new();
        fn visit(
            db: &Db,
            t: TypeDef<'_>,
            bit: i32,
            active: &mut HashSet<Key>,
            slots: &mut Vec<J>,
        ) -> Result<()> {
            ensure!(t.category() == TypeCategory::Interface, "not an interface");
            ensure!(
                !t.flags().contains(TypeAttributes::WindowsRuntime),
                "WinRT requirements cannot be used as native COM base interfaces"
            );
            ensure!(
                t.generic_params().next().is_none(),
                "generic interface vtable ABI is not supported"
            );
            ensure!(active.insert(db.key(t, 2)), "interface cycle");
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
                slots.push(json!({"index":slots.len(),"method":m.name(),"declaring_type":t.name(),"token":format!("0x{:08x}",token(m,6)),"source_id":db.sources[m.to_row().file].id,"id":db.id(db.key(m,6)),"declaring_type_id":db.id(db.key(t,2))}));
            }
            active.remove(&db.key(t, 2));
            Ok(())
        }
        visit(self, t, bit, &mut active, &mut slots)?;
        Ok(
            json!({"name":name,"arch":arch,"id":self.id(key),"source_id":self.sources[key.file].id,"slots":slots}),
        )
    }
}
