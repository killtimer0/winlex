//! Compact C/IDL-like reading view; not a compilable header or an ABI definition.
use std::fmt::Write;
use winlex_core::windows_metadata::reader::*;
use winlex_core::windows_metadata::*;
use winlex_core::{Architecture, Catalog, Declaration, EntityId, Result, guid, integer};

fn ty(t: &Type, name: &str) -> String {
    match t {
        Type::ArrayFixed(t, n) => ty(t, &format!("{name}[{n}]")),
        Type::Array(t) => ty(t, &format!("{name}[]")),
        Type::PtrConst(t, depth) => format!("const {}", ty(&Type::PtrMut(t.clone(), *depth), name)),
        Type::PtrMut(t, depth) => {
            let name = format!("{}{name}", "*".repeat((*depth).min(256)));
            let name = if matches!(**t, Type::ArrayFixed(..) | Type::Array(..)) {
                format!("({name})")
            } else {
                name
            };
            ty(t, &name)
        }
        Type::RefMut(t) => ty(t, &format!("&{name}")),
        Type::RefConst(t) => format!("const {}", ty(t, name)),
        _ => {
            let base = match t {
                Type::ClassName(n) | Type::ValueName(n) => {
                    if n.generics.is_empty() {
                        n.name.to_owned()
                    } else {
                        format!(
                            "{}<{}>",
                            n.name.split('`').next().unwrap_or(&n.name),
                            n.generics
                                .iter()
                                .map(|t| ty(t, ""))
                                .collect::<Vec<_>>()
                                .join(", ")
                        )
                    }
                }
                Type::Generic(n, _) | Type::MethodGeneric(n, _) => n.to_string(),
                Type::Void => "void".into(),
                Type::Bool => "bool".into(),
                Type::I8 => "INT8".into(),
                Type::U8 => "UINT8".into(),
                Type::I16 => "INT16".into(),
                Type::U16 => "UINT16".into(),
                Type::I32 => "INT32".into(),
                Type::U32 => "UINT32".into(),
                Type::I64 => "INT64".into(),
                Type::U64 => "UINT64".into(),
                Type::F32 => "float".into(),
                Type::F64 => "double".into(),
                Type::ISize => "INT_PTR".into(),
                Type::USize => "UINT_PTR".into(),
                Type::Char => "char16_t".into(),
                Type::String => "string".into(),
                Type::Object => "object".into(),
                other => format!("{other:?}"),
            };
            if name.is_empty() {
                base
            } else {
                format!("{base} {name}")
            }
        }
    }
}

/// A semantic numeric region in RichEdit UTF-16 coordinates (one code unit per newline).
#[derive(Clone, Debug)]
pub struct NumberRegion {
    pub range: std::ops::Range<usize>,
    pub value: u64,
    pub unit: Option<&'static str>,
    pub entity: EntityId,
}

#[derive(Default)]
pub struct Rendered {
    pub text: String,
    pub numbers: Vec<NumberRegion>,
    position: usize,
}

impl Write for Rendered {
    fn write_str(&mut self, text: &str) -> std::fmt::Result {
        let text: std::borrow::Cow<'_, str> = if text.contains("\r\n") {
            text.replace("\r\n", "\n").into()
        } else {
            text.into()
        };
        self.position += text.encode_utf16().count();
        self.text.push_str(&text);
        Ok(())
    }
}

impl Rendered {
    fn number(
        &mut self,
        value: u64,
        unit: Option<&'static str>,
        entity: EntityId,
    ) -> std::fmt::Result {
        let start = self.position;
        write!(self, "{value}(0x{value:X})")?;
        if let Some(unit) = unit {
            write!(self, " {unit}")?;
        }
        self.numbers.push(NumberRegion {
            range: start..self.position,
            value,
            unit,
            entity,
        });
        Ok(())
    }

    fn bit_position(&mut self, bits: u64, entity: EntityId) -> std::fmt::Result {
        if bits.is_multiple_of(8) {
            self.number(bits / 8, Some("bytes"), entity)
        } else {
            self.number(bits, Some("bits"), entity)
        }
    }

    fn slot_position(
        &mut self,
        index: usize,
        arch: Architecture,
        entity: EntityId,
    ) -> std::fmt::Result {
        write!(self, "slot ")?;
        self.number(index as u64, None, entity)?;
        write!(self, ", vtable +")?;
        self.number(
            index as u64 * arch.pointer_bytes() as u64,
            Some("bytes"),
            entity,
        )
    }
}

pub fn render(catalog: &Catalog, id: EntityId, arch: Architecture) -> Result<Rendered> {
    let mut out = Rendered::default();
    append(&mut out, catalog, id, arch, "")?;
    Ok(out)
}

fn generic_name<'a>(name: &str, params: impl Iterator<Item = GenericParam<'a>>) -> String {
    let mut params: Vec<_> = params.collect();
    params.sort_by_key(|p| p.sequence());
    if params.is_empty() {
        name.to_owned()
    } else {
        format!(
            "{}<{}>",
            name.split('`').next().unwrap_or(name),
            params
                .iter()
                .map(|p| p.name())
                .collect::<Vec<_>>()
                .join(", ")
        )
    }
}

fn method(out: &mut Rendered, m: MethodDef<'_>, name: &str, pad: &str) -> Result<()> {
    let signature = m.signature(&[]);
    let params = m.params_by_sequence(signature.types.len())?;
    let parameters = signature
        .types
        .iter()
        .zip(params.params())
        .enumerate()
        .map(|(i, (t, p))| {
            let name = p
                .map(|p| p.name().to_owned())
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| format!("arg{}", i + 1));
            let mut flags = Vec::new();
            if let Some(p) = p {
                match p.direction() {
                    ParamDirection::Input => flags.push("in"),
                    ParamDirection::Output => flags.push("out"),
                    ParamDirection::InputOutput => {
                        flags.push("in");
                        flags.push("out");
                    }
                    ParamDirection::Unspecified => {}
                }
                if p.is_optional() {
                    flags.push("optional");
                }
                if p.is_reserved() {
                    flags.push("reserved");
                }
                if p.is_retval_attribute() {
                    flags.push("retval");
                }
                if p.has_attribute("ConstAttribute") {
                    flags.push("const");
                }
            }
            let prefix = if flags.is_empty() {
                String::new()
            } else {
                format!("[{}] ", flags.join(", "))
            };
            format!("{prefix}{}", ty(t, &name))
        })
        .collect::<Vec<_>>();
    let parameters = if parameters.is_empty() {
        "void".into()
    } else {
        format!(
            "\n{pad}    {}\n{pad}",
            parameters.join(&format!(",\n{pad}    "))
        )
    };
    writeln!(
        out,
        "{pad}{};",
        ty(&signature.return_type, &format!("{name}({parameters})"))
    )?;
    Ok(())
}

fn append(
    out: &mut Rendered,
    c: &Catalog,
    id: EntityId,
    arch: Architecture,
    pad: &str,
) -> Result<()> {
    let row = c.declaration(id, Some(arch))?;
    let entity = c.summarize(id, &row);
    match row {
        Declaration::Type(t) => {
            let layout = if entity.kind == "struct" {
                match c.layout(id, arch) {
                    Ok(layout) => {
                        write!(out, "{pad}// [{}] size: ", arch.name())?;
                        out.bit_position(layout.size_bits, id)?;
                        write!(out, ", alignment: ")?;
                        out.bit_position(layout.alignment_bits, id)?;
                        writeln!(out)?;
                        Some(layout)
                    }
                    Err(error) => {
                        writeln!(
                            out,
                            "{pad}// [{}] Layout unavailable: {error:#}",
                            arch.name()
                        )?;
                        None
                    }
                }
            } else {
                None
            };
            let vtable = if entity.kind == "interface" {
                match c.vtable(id, arch) {
                    Ok(vtable) => {
                        write!(out, "{pad}// [{}] vtable: ", arch.name())?;
                        out.number(vtable.slots.len() as u64, None, id)?;
                        writeln!(out, " slots")?;
                        Some(vtable)
                    }
                    Err(error) => {
                        writeln!(
                            out,
                            "{pad}// [{}] Vtable unavailable: {error:#}",
                            arch.name()
                        )?;
                        None
                    }
                }
            } else {
                None
            };
            if let Some(guid) = guid(t) {
                writeln!(out, "{pad}// GUID: {guid}")?;
            }
            let keyword = match entity.kind.as_str() {
                "interface" => "interface",
                "enum" => "enum",
                "class" => "class",
                "delegate" => "delegate",
                _ if t.flags().contains(TypeAttributes::ExplicitLayout) => {
                    "struct /* explicit layout */"
                }
                _ => "struct",
            };
            let name = generic_name(t.name(), t.generic_params());
            if entity.kind == "delegate"
                && let Some(invoke) = t.methods().find(|m| m.name() == "Invoke")
            {
                write!(out, "{pad}delegate ")?;
                method(out, invoke, &name, "")?;
                return Ok(());
            }
            let generics = t.generic_types();
            let mut bases = Vec::new();
            if let Some(base) = t.extends() {
                let base = match base {
                    TypeDefOrRef::TypeSpec(s) => s.ty(&generics),
                    base => Type::ClassName(base.type_name()),
                };
                bases.push(ty(&base, ""));
            }
            bases.extend(t.interface_impls().map(|i| ty(&i.interface(&generics), "")));
            if entity.kind == "enum" {
                bases = t
                    .underlying_type()
                    .map(|t| ty(&t, ""))
                    .into_iter()
                    .collect();
            }
            let bases = if bases.is_empty() {
                String::new()
            } else {
                format!(" : {}", bases.join(", "))
            };
            writeln!(out, "{pad}{keyword} {name}{bases}\n{pad}{{")?;
            let inner = format!("{pad}    ");
            if let Some(vtable) = &vtable {
                for (index, slot) in vtable
                    .slots
                    .iter()
                    .enumerate()
                    .filter(|(_, slot)| slot.declaring_type_id != Some(id))
                {
                    write!(out, "{inner}// ")?;
                    out.slot_position(index, arch, slot.id.unwrap_or(id))?;
                    writeln!(
                        out,
                        ": {}::{} (inherited/implicit)",
                        slot.declaring_type, slot.method
                    )?;
                }
            }
            if entity.kind == "enum" {
                for f in t.fields().filter(|f| f.constant().is_some()) {
                    if winlex_core::metadata::supports(f, arch.bit()) {
                        let value = f.constant().unwrap().value();
                        writeln!(
                            out,
                            "{inner}{} = {},",
                            f.name(),
                            integer(&value)
                                .map(|v| v.to_string())
                                .unwrap_or_else(|| format!("{value:?}"))
                        )?;
                    }
                }
                writeln!(out, "{pad}}};")?;
                return Ok(());
            }
            for member in super::member_ids(t.fields(), 4, Some(arch))
                .chain(super::member_ids(t.properties(), 0x17, Some(arch)))
                .chain(super::member_ids(t.events(), 0x14, Some(arch)))
                .chain(super::member_ids(t.methods(), 6, Some(arch)))
            {
                if let Some(field) = layout
                    .as_ref()
                    .and_then(|layout| layout.fields.iter().find(|f| f.id == member))
                {
                    write!(out, "{inner}// offset: ")?;
                    out.bit_position(field.offset_bits, member)?;
                    write!(out, ", size: ")?;
                    out.bit_position(field.size_bits, member)?;
                    writeln!(out)?;
                    for bitfield in &field.bitfields {
                        write!(out, "{inner}// {}: bit offset ", bitfield.name)?;
                        out.number(
                            field.offset_bits + bitfield.relative_offset_bits,
                            Some("bits"),
                            member,
                        )?;
                        write!(out, " from struct start, width ")?;
                        out.number(bitfield.width_bits, Some("bits"), member)?;
                        writeln!(out)?;
                    }
                }
                if let Some(vtable) = &vtable {
                    for (index, _) in vtable
                        .slots
                        .iter()
                        .enumerate()
                        .filter(|(_, slot)| slot.id == Some(member))
                    {
                        write!(out, "{inner}// ")?;
                        out.slot_position(index, arch, member)?;
                        writeln!(out)?;
                    }
                }
                append(out, c, member, arch, &inner)?;
            }
            writeln!(out, "{pad}}};")?;
        }
        Declaration::Field(f) => {
            if let Some(offset) = f.explicit_offset() {
                write!(out, "{pad}// Declared offset: ")?;
                out.number(offset.into(), Some("bytes"), id)?;
                writeln!(out)?;
            }
            write!(out, "{pad}{}", ty(&f.ty(), f.name()))?;
            if let Some(value) = f.constant() {
                let value = value.value();
                write!(
                    out,
                    " = {}",
                    integer(&value)
                        .map(|v| v.to_string())
                        .unwrap_or_else(|| format!("{value:?}"))
                )?;
            }
            writeln!(out, ";")?;
        }
        Declaration::Method(m) => {
            if let Some(import) = m.impl_map() {
                writeln!(
                    out,
                    "{pad}// {}!{}; {:?}",
                    import.import_scope().name(),
                    import.import_name(),
                    m.calling_convention()
                )?;
            }
            method(out, m, &generic_name(m.name(), m.generic_params()), pad)?;
        }
        Declaration::Property(p) => {
            writeln!(
                out,
                "{pad}{}; // property",
                ty(&p.signature().return_type, p.name())
            )?;
        }
        Declaration::Event(e) => {
            writeln!(out, "{pad}{}; // event", ty(&e.ty(), e.name()))?;
        }
        Declaration::Reference(_) => {
            writeln!(out, "{pad}// Type reference: {}", entity.full_name)?;
            match c.resolve_reference(id, arch) {
                Ok(target) => writeln!(out, "{pad}// Resolved: {}", c.entity(target)?.full_name)?,
                Err(error) => writeln!(out, "{pad}// {error}")?,
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numeric_regions_follow_utf16_and_normalized_newlines() {
        let mut out = Rendered::default();
        write!(out, "// 文😀\r\n").unwrap();
        let id = EntityId {
            source_id: 0,
            token: 0x04000001,
        };
        out.number(64, Some("bytes"), id).unwrap();
        assert_eq!(out.text, "// 文😀\n64(0x40) bytes");
        assert_eq!(out.numbers[0].range, 7..21);
        assert_eq!(out.numbers[0].entity, id);
    }
}
