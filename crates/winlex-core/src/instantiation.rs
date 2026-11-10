//! A declaration's metadata identity and optional closed owner-type arguments.
use crate::{Architecture, Catalog, EntityId, Result};
use anyhow::{bail, ensure};
use windows_metadata::{Signature, Type, TypeName};

/// Arguments belong to the declaration's containing type, never to its method.
/// An empty list selects the metadata definition (including open generic definitions).
#[derive(Clone, Debug, PartialEq)]
pub struct DeclarationTarget {
    pub id: EntityId,
    pub arguments: Vec<Type>,
}

impl From<EntityId> for DeclarationTarget {
    fn from(id: EntityId) -> Self {
        Self {
            id,
            arguments: vec![],
        }
    }
}

impl DeclarationTarget {
    pub fn with_id(&self, id: EntityId) -> Self {
        Self {
            id,
            arguments: self.arguments.clone(),
        }
    }

    /// Substitute VAR recursively; MVAR belongs to a different parameter scope.
    pub fn ty(&self, ty: &Type) -> Result<Type> {
        transform(ty, 0, &mut |ty| {
            if let Type::Generic(_, index) = ty
                && !self.arguments.is_empty()
            {
                return self.arguments.get(*index as usize).cloned().ok_or_else(|| {
                    anyhow::anyhow!("type parameter index {index} exceeds argument count")
                });
            }
            Ok(ty.clone())
        })
    }

    pub fn signature(&self, mut signature: Signature) -> Result<Signature> {
        signature.return_type = self.ty(&signature.return_type)?;
        signature.types = signature
            .types
            .iter()
            .map(|ty| self.ty(ty))
            .collect::<Result<_>>()?;
        Ok(signature)
    }
}

fn transform(
    ty: &Type,
    depth: usize,
    leaf: &mut impl FnMut(&Type) -> Result<Type>,
) -> Result<Type> {
    ensure!(depth < 128, "type signature depth exceeds limit");
    let next = depth + 1;
    Ok(match ty {
        Type::ClassName(name) | Type::ValueName(name) => {
            let mut name = name.clone();
            name.generics = name
                .generics
                .iter()
                .map(|ty| transform(ty, next, leaf))
                .collect::<Result<_>>()?;
            let ty = if matches!(ty, Type::ClassName(_)) {
                Type::ClassName(name)
            } else {
                Type::ValueName(name)
            };
            leaf(&ty)?
        }
        Type::Array(ty) => Type::Array(Box::new(transform(ty, next, leaf)?)),
        Type::ArrayFixed(ty, count) => {
            Type::ArrayFixed(Box::new(transform(ty, next, leaf)?), *count)
        }
        Type::PtrMut(ty, depth) => Type::PtrMut(Box::new(transform(ty, next, leaf)?), *depth),
        Type::PtrConst(ty, depth) => Type::PtrConst(Box::new(transform(ty, next, leaf)?), *depth),
        Type::RefMut(ty) => Type::RefMut(Box::new(transform(ty, next, leaf)?)),
        Type::RefConst(ty) => Type::RefConst(Box::new(transform(ty, next, leaf)?)),
        _ => leaf(ty)?,
    })
}

impl Catalog {
    /// Validate and canonicalize a target in this catalog snapshot.
    pub fn bind_target(
        &self,
        target: &DeclarationTarget,
        arch: Architecture,
    ) -> Result<DeclarationTarget> {
        let declaration = self.declaration(target.id, Some(arch))?;
        if target.arguments.is_empty() {
            return Ok(target.clone());
        }
        let owner = declaration
            .owner()
            .ok_or_else(|| anyhow::anyhow!("type reference cannot own arguments"))?;
        ensure!(
            target.arguments.len() == owner.generic_params().len(),
            "generic argument count mismatch"
        );
        Ok(DeclarationTarget {
            id: target.id,
            arguments: target
                .arguments
                .iter()
                .map(|ty| self.bind_argument(target.id, ty, arch))
                .collect::<Result<_>>()?,
        })
    }

    pub(crate) fn bind_argument(
        &self,
        owner: EntityId,
        ty: &Type,
        arch: Architecture,
    ) -> Result<Type> {
        transform(ty, 0, &mut |ty| {
            match ty {
                Type::Generic(..) | Type::MethodGeneric(..) => {
                    bail!("instance arguments must be closed types")
                }
                Type::Void => bail!("void is not a type argument"),
                Type::ValueName(name) if self.is_guid_type(name) => {
                    Ok(Type::value_named("System", "Guid"))
                }
                Type::ClassName(name) | Type::ValueName(name) => {
                    let mut name = name.clone();
                    match self.resolve_type(owner, &name, arch) {
                        Ok(id) => {
                            let row = self.declaration(id, Some(arch))?;
                            let definition = row.owner().expect("resolved TypeDef");
                            ensure!(
                                name.generics.len() == definition.generic_params().len(),
                                "generic argument count mismatch"
                            );
                            name.namespace = definition.namespace().into();
                            name.name = definition.name().into();
                            name.identity = Some((id.source_id, id.token));
                            return Ok(
                                if matches!(
                                    definition.category(),
                                    windows_metadata::reader::TypeCategory::Struct
                                        | windows_metadata::reader::TypeCategory::Enum
                                ) {
                                    Type::ValueName(name)
                                } else {
                                    Type::ClassName(name)
                                },
                            );
                        }
                        // Keep missing/ambiguous dependencies source-scoped rather than
                        // guessing a same-named definition in another loaded source.
                        Err(error)
                            if name.identity.is_none_or(|(_, token)| token >> 24 != 1)
                                || error.is::<crate::metadata::UnavailableArchitecture>() =>
                        {
                            return Err(error);
                        }
                        Err(_) => {}
                    }
                    Ok(if matches!(ty, Type::ClassName(_)) {
                        Type::ClassName(name)
                    } else {
                        Type::ValueName(name)
                    })
                }
                _ => Ok(ty.clone()),
            }
        })
    }

    /// Resolve a signature link. Open uses navigate to the definition, since their
    /// parameters belong to the source declaration and cannot escape into another owner.
    pub fn resolve_target(
        &self,
        owner: EntityId,
        name: &TypeName,
        arch: Architecture,
    ) -> Result<DeclarationTarget> {
        let id = self.resolve_type(owner, name, arch)?;
        let row = self.declaration(id, Some(arch))?;
        let definition = row.owner().expect("resolved TypeDef");
        if name.generics.is_empty() {
            return Ok(id.into());
        }
        ensure!(
            name.generics.len() == definition.generic_params().len(),
            "generic argument count mismatch"
        );
        let mut open = false;
        for argument in &name.generics {
            transform(argument, 0, &mut |ty| {
                open |= matches!(ty, Type::Generic(..) | Type::MethodGeneric(..));
                Ok(ty.clone())
            })?;
        }
        if open {
            return Ok(id.into());
        }
        Ok(DeclarationTarget {
            id,
            arguments: name
                .generics
                .iter()
                .map(|ty| self.bind_argument(owner, ty, arch))
                .collect::<Result<_>>()?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Declaration,
        test_support::{catalog, type_id},
    };
    use windows_metadata::{writer, *};

    #[test]
    fn closed_owner_arguments_substitute_nested_types_but_not_method_parameters() {
        let mut file = writer::File::new("GenericInstances");
        let box_type = file.TypeDef(
            "Fixture",
            "Box`1",
            writer::TypeDefOrRef::default(),
            TypeAttributes::Public,
        );
        file.GenericParam(
            "T",
            writer::TypeOrMethodDef::TypeDef(box_type),
            0,
            GenericParamAttributes(0),
        );
        file.Field(
            "Value",
            &Type::Generic("T".into(), 0),
            FieldAttributes::Public,
        );
        let catalog = catalog("generic-instances", file);
        let id = type_id(&catalog, "Box`1");
        let target = catalog
            .bind_target(
                &DeclarationTarget {
                    id,
                    arguments: vec![Type::I32],
                },
                Architecture::X64,
            )
            .unwrap();
        let Declaration::Type(row) = catalog.declaration(id, None).unwrap() else {
            panic!()
        };
        assert_eq!(
            target.ty(&row.fields().next().unwrap().ty()).unwrap(),
            Type::I32
        );
        let signature = target
            .signature(Signature {
                flags: MethodCallAttributes(0x30),
                return_type: Type::MethodGeneric("U".into(), 0),
                types: vec![
                    Type::Generic("T".into(), 0),
                    Type::Array(Box::new(Type::Generic("T".into(), 0))),
                ],
            })
            .unwrap();
        assert_eq!(signature.return_type, Type::MethodGeneric("U".into(), 0));
        assert_eq!(
            signature.types,
            [Type::I32, Type::Array(Box::new(Type::I32))]
        );
        let mut nested = TypeName::named("Fixture", "Box`1");
        nested.generics = vec![Type::RefConst(Box::new(Type::PtrMut(
            Box::new(Type::Generic("T".into(), 0)),
            1,
        )))];
        let Type::ClassName(result) = target.ty(&Type::ClassName(nested)).unwrap() else {
            panic!()
        };
        assert_eq!(
            result.generics,
            [Type::RefConst(Box::new(Type::PtrMut(
                Box::new(Type::I32),
                1
            )))]
        );
        for arguments in [
            vec![Type::I32, Type::I64],
            vec![Type::Generic("T".into(), 0)],
            vec![Type::Void],
        ] {
            assert!(
                catalog
                    .bind_target(&DeclarationTarget { id, arguments }, Architecture::X64)
                    .is_err()
            );
        }
        assert!(target.ty(&Type::Generic("bad".into(), 1)).is_err());
        let mut deep = Type::I32;
        for _ in 0..128 {
            deep = Type::Array(Box::new(deep));
        }
        assert!(target.ty(&deep).is_err());
    }

    #[test]
    fn signature_navigation_canonicalizes_named_arguments_and_keeps_open_uses_at_definitions() {
        let mut file = writer::File::new("GenericNavigation");
        file.TypeDef(
            "Fixture",
            "Item",
            writer::TypeDefOrRef::default(),
            TypeAttributes::Public,
        );
        let box_type = file.TypeDef(
            "Fixture",
            "Box`1",
            writer::TypeDefOrRef::default(),
            TypeAttributes::Public,
        );
        file.GenericParam(
            "T",
            writer::TypeOrMethodDef::TypeDef(box_type),
            0,
            GenericParamAttributes(0),
        );
        file.TypeDef(
            "Fixture",
            "Uses",
            writer::TypeDefOrRef::default(),
            TypeAttributes::Public,
        );
        let mut named = TypeName::named("Fixture", "Box`1");
        named.generics = vec![Type::class_named("Fixture", "Item")];
        file.Field("Member", &Type::ClassName(named), FieldAttributes::Public);
        let mut guid_box = TypeName::named("Fixture", "Box`1");
        guid_box.generics = vec![Type::value_named("System", "Guid")];
        file.Field(
            "GuidMember",
            &Type::ClassName(guid_box),
            FieldAttributes::Public,
        );
        let catalog = catalog("generic-navigation", file);
        let owner = type_id(&catalog, "Uses");
        let Declaration::Type(row) = catalog.declaration(owner, None).unwrap() else {
            panic!()
        };
        let Type::ClassName(mut name) = row.fields().next().unwrap().ty() else {
            panic!()
        };
        let Type::ClassName(guid_link) = row.fields().nth(1).unwrap().ty() else {
            panic!()
        };
        let guid_target = catalog
            .resolve_target(owner, &guid_link, Architecture::X64)
            .unwrap();
        assert_eq!(
            catalog
                .parse_target("Fixture.Box<guid>", Architecture::X64)
                .unwrap(),
            guid_target
        );
        let target = catalog
            .resolve_target(owner, &name, Architecture::X64)
            .unwrap();
        assert_eq!(target.id, type_id(&catalog, "Box`1"));
        assert_eq!(
            catalog
                .parse_target("Fixture.Box<Fixture.Item>", Architecture::X64)
                .unwrap(),
            target
        );
        assert_eq!(
            catalog
                .parse_argument(target.id, "Fixture.Item", Architecture::X64)
                .unwrap(),
            target.arguments[0]
        );
        for (expression, arguments) in [
            ("guid", vec![Type::value_named("System", "Guid")]),
            ("int32", vec![Type::I32]),
            (
                "Fixture.Box<Fixture.Item>",
                vec![Type::ClassName(name.clone())],
            ),
        ] {
            let mut link = name.clone();
            link.generics = arguments;
            let linked = catalog
                .resolve_target(owner, &link, Architecture::X64)
                .unwrap();
            assert_eq!(
                catalog
                    .parse_argument(target.id, expression, Architecture::X64)
                    .unwrap(),
                linked.arguments[0]
            );
            assert_eq!(
                catalog
                    .parse_target(&format!("Fixture.Box<{expression}>"), Architecture::X64)
                    .unwrap(),
                linked
            );
        }
        let Type::ClassName(argument) = &target.arguments[0] else {
            panic!()
        };
        let item = type_id(&catalog, "Item");
        assert_eq!(argument.identity, Some((item.source_id, item.token)));
        assert_eq!(
            catalog.bind_target(&target, Architecture::X64).unwrap(),
            target
        );
        for arch in Architecture::ALL {
            assert_eq!(
                catalog.bind_target(&target, arch).unwrap(),
                catalog
                    .parse_target("Fixture.Box<Fixture.Item>", arch)
                    .unwrap()
            );
        }
        name.generics = vec![Type::Array(Box::new(Type::Generic("T".into(), 0)))];
        assert_eq!(
            catalog
                .resolve_target(owner, &name, Architecture::X64)
                .unwrap(),
            target.id.into()
        );
        name.generics = vec![Type::I32, Type::I64];
        assert!(
            catalog
                .resolve_target(owner, &name, Architecture::X64)
                .is_err()
        );
    }

    #[test]
    fn instance_arguments_keep_their_source_when_the_owner_changes() {
        use crate::test_support::source;
        let item = |assembly| {
            let mut file = writer::File::new(assembly);
            file.TypeDef(
                "Shared",
                "Item",
                writer::TypeDefOrRef::default(),
                TypeAttributes::Public,
            );
            file.Field(
                "Missing",
                &Type::class_named("Missing", "Dependency"),
                FieldAttributes::Public,
            );
            source(assembly, file)
        };
        let mut file = writer::File::new("Containers");
        let box_type = file.TypeDef(
            "Fixture",
            "Box`1",
            writer::TypeDefOrRef::default(),
            TypeAttributes::Public,
        );
        file.GenericParam(
            "T",
            writer::TypeOrMethodDef::TypeDef(box_type),
            0,
            GenericParamAttributes(0),
        );
        let catalog =
            Catalog::from_inputs([item("First"), item("Second"), source("Containers", file)])
                .unwrap();
        let id = type_id(&catalog, "Box`1");
        let item_ids: Vec<_> = catalog
            .search_exact(
                "Shared.Item",
                crate::SearchOptions {
                    categories: Some(crate::EntityCategory::Type as u32),
                    ..Default::default()
                },
            )
            .unwrap()
            .map(|item| item.unwrap().id)
            .collect();
        assert_eq!(item_ids.len(), 2);
        let targets: Vec<_> = item_ids
            .iter()
            .map(|item| {
                let mut name = TypeName::named("Shared", "Item");
                name.identity = Some((item.source_id, item.token));
                catalog
                    .bind_target(
                        &DeclarationTarget {
                            id,
                            arguments: vec![Type::ClassName(name)],
                        },
                        Architecture::X64,
                    )
                    .unwrap()
            })
            .collect();
        assert_ne!(targets[0], targets[1]);
        assert!(
            catalog
                .parse_argument(id, "Shared.Item", Architecture::X64)
                .is_err()
        );
        for (target, expected) in targets.iter().zip(item_ids) {
            let Declaration::Type(owner) = catalog.declaration(expected, None).unwrap() else {
                panic!()
            };
            let unresolved = owner.fields().next().unwrap().ty();
            let mut link = TypeName::named("Fixture", "Box`1");
            link.identity = Some((id.source_id, id.token));
            link.generics = vec![unresolved.clone()];
            let linked = catalog
                .resolve_target(expected, &link, Architecture::X64)
                .unwrap();
            assert_eq!(linked.id, id);
            assert_eq!(linked.arguments, [unresolved]);
            assert_eq!(
                catalog.bind_target(&linked, Architecture::X64).unwrap(),
                linked
            );
            let expression = catalog
                .argument_expression(id, &target.arguments[0], Architecture::X64)
                .unwrap();
            assert_eq!(
                catalog
                    .parse_argument(id, &expression, Architecture::X64)
                    .unwrap(),
                target.arguments[0]
            );
            for selector in [
                format!("{}::Shared.Item", expected.source_id),
                format!("{}::0x{:08x}", expected.source_id, expected.token),
            ] {
                assert_eq!(
                    catalog
                        .select_entity(&selector, Some(Architecture::X64), true)
                        .unwrap(),
                    expected
                );
                assert_eq!(
                    catalog
                        .parse_target(&format!("Fixture.Box<{selector}>"), Architecture::X64)
                        .unwrap(),
                    *target
                );
            }
            let (Type::ClassName(argument) | Type::ValueName(argument)) = &target.arguments[0]
            else {
                panic!()
            };
            assert_eq!(
                argument.identity,
                Some((expected.source_id, expected.token))
            );
            let Type::ClassName(name) = target.ty(&Type::Generic("T".into(), 0)).unwrap() else {
                panic!()
            };
            assert_eq!(
                catalog
                    .resolve_target(id, &name, Architecture::X64)
                    .unwrap()
                    .id,
                expected
            );
        }
    }
}
