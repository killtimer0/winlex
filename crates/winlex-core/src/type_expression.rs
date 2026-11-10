//! Closed type expressions shared by interactive binding and the CLI.
use crate::{Architecture, Catalog, Declaration, DeclarationTarget, EntityId, Result};
use anyhow::{bail, ensure};
use windows_metadata::{Type, TypeName, reader::TypeCategory};

const PRIMITIVES: &[(&str, Type)] = &[
    ("bool", Type::Bool),
    ("char", Type::Char),
    ("int8", Type::I8),
    ("uint8", Type::U8),
    ("int16", Type::I16),
    ("uint16", Type::U16),
    ("int32", Type::I32),
    ("uint32", Type::U32),
    ("int64", Type::I64),
    ("uint64", Type::U64),
    ("isize", Type::ISize),
    ("usize", Type::USize),
    ("float32", Type::F32),
    ("float64", Type::F64),
    ("string", Type::String),
    ("object", Type::Object),
];

impl Catalog {
    fn parse_closed(&self, expression: &str, arch: Architecture, trailing: &str) -> Result<Type> {
        let mut parser = Parser {
            catalog: self,
            text: expression,
            arch,
        };
        let ty = parser.ty(0)?;
        ensure!(parser.text.trim().is_empty(), "{trailing}");
        Ok(ty)
    }

    /// Parse one closed type argument, preserving the owner's source context.
    pub fn parse_argument(
        &self,
        owner: EntityId,
        expression: &str,
        arch: Architecture,
    ) -> Result<Type> {
        let ty = self.parse_closed(expression, arch, "unexpected text after type argument")?;
        self.bind_argument(owner, &ty, arch)
    }

    /// Source-qualified editable spelling, with tokens for otherwise ambiguous names.
    pub fn argument_expression(
        &self,
        owner: EntityId,
        ty: &Type,
        arch: Architecture,
    ) -> Result<String> {
        if let Some((name, _)) = PRIMITIVES.iter().find(|(_, primitive)| primitive == ty) {
            return Ok((*name).into());
        }
        Ok(match ty {
            Type::ValueName(name) if self.is_guid_type(name) => "guid".into(),
            Type::ClassName(name) | Type::ValueName(name) => {
                let id = self.resolve_type(owner, name, arch).or_else(|error| {
                    name.identity
                        .map(|(source_id, token)| EntityId { source_id, token })
                        .ok_or(error)
                })?;
                let qualified = self
                    .entity(id)
                    .ok()
                    .map(|entity| format!("{}::{}", id.source_id, entity.full_name));
                let mut value = qualified
                    .filter(|value| {
                        !value
                            .chars()
                            .any(|c| c.is_whitespace() || matches!(c, '<' | '>' | ',' | '[' | ']'))
                            && self
                                .select_entity(value, Some(arch), true)
                                .is_ok_and(|selected| selected == id)
                    })
                    .unwrap_or_else(|| id.to_string());
                if !name.generics.is_empty() {
                    let arguments = name
                        .generics
                        .iter()
                        .map(|ty| self.argument_expression(owner, ty, arch))
                        .collect::<Result<Vec<_>>>()?;
                    value.push('<');
                    value.push_str(&arguments.join(", "));
                    value.push('>');
                }
                value
            }
            Type::Array(inner) => format!("{}[]", self.argument_expression(owner, inner, arch)?),
            _ => bail!("type argument has no editable closed-type expression"),
        })
    }

    pub fn type_parameter_names(&self, id: EntityId, arch: Architecture) -> Result<Vec<String>> {
        let row = self.declaration(id, Some(arch))?;
        let Some(owner) = row.owner() else {
            return Ok(vec![]);
        };
        let mut params: Vec<_> = owner.generic_params().collect();
        params.sort_by_key(|p| p.sequence());
        Ok(params.iter().map(|p| p.name().to_owned()).collect())
    }

    /// Parse `Namespace.Type<int32, SOURCE::Other>`; names are exact and case-sensitive.
    pub fn parse_target(&self, expression: &str, arch: Architecture) -> Result<DeclarationTarget> {
        let ty = self.parse_closed(expression, arch, "unexpected text after type expression")?;
        let (Type::ClassName(name) | Type::ValueName(name)) = ty else {
            bail!("select a named metadata type")
        };
        let (source_id, token) = name
            .identity
            .ok_or_else(|| anyhow::anyhow!("select a named metadata type"))?;
        self.bind_target(
            &DeclarationTarget {
                id: EntityId { source_id, token },
                arguments: name.generics,
            },
            arch,
        )
    }
}

struct Parser<'a> {
    catalog: &'a Catalog,
    text: &'a str,
    arch: Architecture,
}
impl Parser<'_> {
    fn take(&mut self, token: &str) -> bool {
        self.text = self.text.trim_start();
        if let Some(rest) = self.text.strip_prefix(token) {
            self.text = rest;
            true
        } else {
            false
        }
    }
    fn arguments(&mut self, depth: usize) -> Result<Vec<Type>> {
        let mut arguments = vec![];
        loop {
            ensure!(arguments.len() < 64, "too many type arguments");
            arguments.push(self.ty(depth + 1)?);
            if !self.take(",") {
                break;
            }
        }
        ensure!(self.take(">"), "expected > after type arguments");
        Ok(arguments)
    }
    fn ty(&mut self, depth: usize) -> Result<Type> {
        ensure!(depth < 64, "type expression depth exceeds limit");
        self.text = self.text.trim_start();
        let end = self
            .text
            .find(|c: char| c.is_whitespace() || matches!(c, '<' | '>' | ',' | '[' | ']'))
            .unwrap_or(self.text.len());
        let name = &self.text[..end];
        ensure!(!name.is_empty(), "expected a closed type");
        self.text = &self.text[end..];
        let arguments = if self.take("<") {
            self.arguments(depth)?
        } else {
            vec![]
        };
        let ty = match PRIMITIVES.iter().find(|(alias, _)| *alias == name) {
            Some((_, ty)) => ty.clone(),
            None if name == "guid" => Type::value_named("System", "Guid"),
            _ => {
                let bare = name.split_once("::").map_or(name, |(_, name)| name);
                let lookup =
                    if !arguments.is_empty() && !bare.contains('`') && !bare.starts_with("0x") {
                        format!("{name}`{}", arguments.len())
                    } else {
                        name.into()
                    };
                let id = self.catalog.select_entity(&lookup, Some(self.arch), true)?;
                let Declaration::Type(row) = self.catalog.declaration(id, Some(self.arch))? else {
                    bail!("expected a type definition")
                };
                ensure!(
                    row.generic_params().len() == arguments.len(),
                    "generic argument count mismatch; open types are not arguments"
                );
                let mut name = TypeName::named(row.namespace(), row.name());
                name.identity = Some((id.source_id, id.token));
                name.generics = arguments;
                return self.array(
                    if matches!(row.category(), TypeCategory::Struct | TypeCategory::Enum) {
                        Type::ValueName(name)
                    } else {
                        Type::ClassName(name)
                    },
                );
            }
        };
        ensure!(
            arguments.is_empty(),
            "primitive types cannot have arguments"
        );
        self.array(ty)
    }
    fn array(&mut self, mut ty: Type) -> Result<Type> {
        let mut depth = 0;
        while self.take("[") {
            depth += 1;
            ensure!(depth < 64, "array depth exceeds limit");
            ensure!(self.take("]"), "only [] arrays are supported");
            ty = Type::Array(Box::new(ty));
        }
        Ok(ty)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{catalog, type_id};
    use windows_metadata::{writer, *};

    #[test]
    fn closed_expressions_preserve_nested_source_identity_and_reject_open_or_trailing_input() {
        let mut file = writer::File::new("Expressions");
        file.TypeDef(
            "Fixture",
            "Item",
            writer::TypeDefOrRef::default(),
            TypeAttributes::Public,
        );
        let container = file.TypeDef(
            "Fixture",
            "Box`1",
            writer::TypeDefOrRef::default(),
            TypeAttributes::Public,
        );
        file.GenericParam(
            "T",
            writer::TypeOrMethodDef::TypeDef(container),
            0,
            GenericParamAttributes(0),
        );
        let catalog = catalog("expressions", file);
        let arch = Architecture::X64;
        let target = catalog
            .parse_target(
                " Fixture.Box<0::Fixture.Box<Expressions::Fixture.Item>> ",
                arch,
            )
            .unwrap();
        let Type::ClassName(inner) = &target.arguments[0] else {
            panic!()
        };
        let Type::ClassName(item) = &inner.generics[0] else {
            panic!()
        };
        let expected = type_id(&catalog, "Item");
        assert_eq!(item.identity, Some((expected.source_id, expected.token)));
        for expression in [
            "bool",
            "char",
            "int8",
            "uint8",
            "int16",
            "uint16",
            "int32",
            "uint32",
            "int64",
            "uint64",
            "isize",
            "usize",
            "float32",
            "float64",
            "string",
            "object",
            "guid",
            "string[]",
            "Box<Fixture.Item>",
            "Box<Box<int32[]>>",
        ] {
            let argument = catalog.parse_argument(target.id, expression, arch).unwrap();
            let editable = catalog
                .argument_expression(target.id, &argument, arch)
                .unwrap();
            assert_eq!(
                catalog.parse_argument(target.id, &editable, arch).unwrap(),
                argument
            );
        }
        for expression in ["", "int32, string", "Box", "int32 junk"] {
            assert!(catalog.parse_argument(target.id, expression, arch).is_err());
        }
        for expression in [
            "guid",
            "Box",
            "Box<>",
            "Box<int32,>",
            "Box<int32>>",
            "Box<int32,string>",
            "Box<Box>",
            "Box<void>",
            "Box<int32> junk",
            "int32<string>",
            "Box<int32[2]>",
            "Box<0::missing>",
        ] {
            assert!(
                catalog.parse_target(expression, arch).is_err(),
                "{expression}"
            );
        }
        assert!(
            catalog
                .parse_target(
                    &format!("{}int32{}", "Box<".repeat(65), ">".repeat(65)),
                    arch
                )
                .is_err()
        );
    }
}
