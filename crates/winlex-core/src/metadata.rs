use crate::source::{AssemblyIdentity, EntityId, Source};
use crate::{Architecture, Bitfield, Slot, Vtable};
use anyhow::{Result, bail, ensure};
use std::collections::HashSet;
use windows_metadata::{reader::*, *};

pub struct Db {
    pub inputs: Vec<crate::SourceData>,
    pub index: Index,
    pub sources: Vec<Source>,
}

pub fn token<'a, R: AsRow<'a>>(r: R, table: u32) -> u32 {
    table << 24 | (r.pos() + 1) as u32
}
pub fn key<'a>(row: impl AsRow<'a>, table: u32) -> EntityId {
    EntityId {
        source_id: row.to_row().file,
        token: token(row, table),
    }
}
pub fn supports<'a>(r: impl HasAttributes<'a>, arch: i32) -> bool {
    r.arches() == 0 || r.arches() & arch != 0
}

pub fn bitfields(f: Field<'_>) -> Result<Vec<Bitfield>> {
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
        result.push(Bitfield {
            name: name.clone(),
            relative_offset_bits: num(&values[1].1)?,
            width_bits: num(&values[2].1)?,
        });
    }
    Ok(result)
}

impl Db {
    pub fn from_inputs(data: impl IntoIterator<Item = crate::SourceData>) -> Result<Self> {
        let inputs: Vec<_> = data.into_iter().collect();
        ensure!(!inputs.is_empty(), "at least one WinMD is required");
        let mut files = vec![];
        let mut sources = vec![];
        for (id, input) in inputs.iter().enumerate() {
            let path = &input.path;
            files.push(
                File::new_shared(input.bytes.clone())
                    .ok_or_else(|| anyhow::anyhow!("invalid metadata: {}", path.display()))?,
            );
            sources.push(Source {
                id,
                path: path.clone(),
                assembly: None,
                references: vec![],
            });
        }
        let index = Index::new(files);
        for (file, source) in sources.iter_mut().enumerate() {
            let mut assemblies = index.rows::<Assembly>(file);
            source.assembly = assemblies.next().map(AssemblyIdentity::definition);
            ensure!(
                assemblies.next().is_none(),
                "multiple Assembly rows in {}",
                source.path.display()
            );
            source.references = index
                .rows::<AssemblyRef>(file)
                .map(AssemblyIdentity::reference)
                .collect();
        }
        Ok(Self {
            inputs,
            index,
            sources,
        })
    }
    pub(crate) fn row<'a, R: AsRow<'a> + 'a>(&'a self, key: EntityId, table: u32) -> Result<R> {
        ensure!(
            key.source_id < self.sources.len() && key.token >> 24 == table,
            "invalid source or token table"
        );
        let pos = (key.token & 0xffffff)
            .checked_sub(1)
            .ok_or_else(|| anyhow::anyhow!("nil token"))? as usize;
        ensure!(
            pos < self.index.rows::<R>(key.source_id).len(),
            "out-of-range token {}",
            key
        );
        Ok(R::from_row(Row {
            index: &self.index,
            file: key.source_id,
            pos,
        }))
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
                let t: TypeDef = self.row(
                    EntityId {
                        source_id: file,
                        token: tok,
                    },
                    2,
                )?;
                ensure!(
                    supports(t, arch),
                    "TypeDef reference unavailable on architecture"
                );
                return Ok(t);
            }
            ensure!(tok >> 24 == 1, "unhandled signature reference token");
            let r: TypeRef = self.row(
                EntityId {
                    source_id: file,
                    token: tok,
                },
                1,
            )?;
            let scope = r.scope_token();
            if scope >> 24 == 1 {
                let parent: TypeRef = self.row(
                    EntityId {
                        source_id: file,
                        token: scope,
                    },
                    1,
                )?;
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
                let reference: AssemblyRef = self.row(
                    EntityId {
                        source_id: file,
                        token: scope,
                    },
                    0x23,
                )?;
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
    pub(crate) fn vtable_key(&self, key: EntityId, arch: Architecture) -> Result<Vtable> {
        let bit = arch.bit();
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
                    slots.push(Slot {
                        method: (*method).into(),
                        declaring_type: declaring_type.into(),
                        id: None,
                        declaring_type_id: None,
                    });
                }
            }
            // WinRT InterfaceImpl entries are requirements, not vtable inheritance.
            for m in t
                .methods()
                .filter(|m| supports(*m, bit) && (!delegate || m.name() == "Invoke"))
            {
                ensure!(
                    m.generic_params().next().is_none(),
                    "generic method ABI unsupported"
                );
                slots.push(method_slot(t, m)?);
            }
            ensure!(
                !delegate || slots.len() == 4,
                "WinRT delegate must have one Invoke method"
            );
            return Ok(Vtable {
                id: key,
                name: name.into(),
                architecture: arch,
                windows_runtime: true,
                slots,
            });
        }
        let mut active = HashSet::new();
        fn visit(
            db: &Db,
            t: TypeDef<'_>,
            bit: i32,
            active: &mut HashSet<EntityId>,
            slots: &mut Vec<Slot>,
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
            ensure!(active.insert(crate::metadata::key(t, 2)), "interface cycle");
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
                slots.push(method_slot(t, m)?);
            }
            active.remove(&crate::metadata::key(t, 2));
            Ok(())
        }
        visit(self, t, bit, &mut active, &mut slots)?;
        Ok(Vtable {
            id: key,
            name: name.into(),
            architecture: arch,
            windows_runtime: false,
            slots,
        })
    }
}
fn method_slot(t: TypeDef<'_>, m: MethodDef<'_>) -> Result<Slot> {
    ensure!(
        !m.flags().contains(MethodAttributes::Static),
        "static interface method unsupported"
    );
    Ok(Slot {
        method: m.name().into(),
        declaring_type: t.name().into(),
        id: Some(key(m, 6)),
        declaring_type_id: Some(key(t, 2)),
    })
}
