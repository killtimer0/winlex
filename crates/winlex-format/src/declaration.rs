//! Compact C/C++-like reading view; not a compilable header or an ABI definition.
use super::{const_annotation, normalized_integer};
use std::fmt::Write;
use winlex_core::windows_metadata::reader::*;
use winlex_core::windows_metadata::*;
use winlex_core::{
    Architecture, Catalog, Declaration, DeclarationTarget, EntityCategory, EntityId, EntitySummary,
    Result, guid, integer, primitive_name,
};

struct Types<'a> {
    catalog: &'a Catalog,
    owner: EntityId,
    arch: Architecture,
    qualified: bool,
}

impl Types<'_> {
    fn ty(&self, t: &Type, name: Rendered) -> Rendered {
        self.qualified_ty(t, name, false)
    }

    fn qualified_ty(&self, t: &Type, name: Rendered, is_const: bool) -> Rendered {
        match t {
            Type::ArrayFixed(t, n) => {
                self.qualified_ty(t, name.suffix(&format!("[{n}]")), is_const)
            }
            Type::Array(t) => self.qualified_ty(t, name.suffix("[]"), is_const),
            pointer @ (Type::PtrMut(t, depth) | Type::PtrConst(t, depth)) => {
                let pointee_const = matches!(pointer, Type::PtrConst(..));
                let mut name = name;
                for level in 0..(*depth).min(256) {
                    let pointer_const = if level == 0 { is_const } else { pointee_const };
                    name = name.prefix(if pointer_const { "*const " } else { "*" });
                }
                let name = if matches!(**t, Type::ArrayFixed(..) | Type::Array(..)) {
                    name.prefix("(").suffix(")")
                } else {
                    name
                };
                self.qualified_ty(t, name, pointee_const)
            }
            Type::RefMut(t) => {
                let rendered = self.ty(t, name.prefix("&"));
                if is_const {
                    rendered.prefix("const ")
                } else {
                    rendered
                }
            }
            Type::RefConst(t) => {
                let rendered = self.ty(t, name).prefix("const ");
                if is_const {
                    rendered.prefix("const ")
                } else {
                    rendered
                }
            }
            _ => {
                let mut base = match t {
                    Type::ClassName(n) | Type::ValueName(n) => self.named(n),
                    Type::Generic(n, _) | Type::MethodGeneric(n, _) => n.to_string().into(),
                    other => primitive_name(other)
                        .map_or_else(|| format!("{other:?}").into(), Into::into),
                };
                if base.regions.is_empty() && !base.text.is_empty() {
                    base.regions.push(Region {
                        range: 0..base.position,
                        kind: RegionKind::Type,
                    });
                }
                if is_const {
                    base = base.prefix("const ");
                }
                if name.text.is_empty() {
                    base
                } else {
                    let mut base = base.suffix(" ");
                    base.fragment(name);
                    base
                }
            }
        }
    }

    /// Type name with its own reference region and argument regions, shared by
    /// signatures and the declaring type of a method.
    fn named(&self, n: &TypeName) -> Rendered {
        let head = if n.generics.is_empty() {
            n.name.as_str()
        } else {
            n.name.split('`').next().unwrap_or(&n.name)
        };
        let kind = self
            .catalog
            .resolve_target(self.owner, n, self.arch)
            .map(RegionKind::Reference)
            .unwrap_or(RegionKind::Type);
        let mut base = if self.qualified && !n.namespace.is_empty() {
            Rendered::from(format!("{}.{head}", n.namespace))
        } else {
            Rendered::from(head)
        };
        base.regions.push(Region {
            range: 0..base.position,
            kind,
        });
        if !n.generics.is_empty() {
            base.write_str("<").unwrap();
            base.fragment(Rendered::join(
                n.generics.iter().map(|t| self.ty(t, "".into())),
                ", ",
            ));
            base.write_str(">").unwrap();
        }
        base
    }
}

/// A semantic region in RichEdit UTF-16 coordinates (one code unit per newline).
#[derive(Clone, Debug)]
pub struct Region {
    pub range: std::ops::Range<usize>,
    pub kind: RegionKind,
}

#[derive(Clone, Debug)]
pub enum RegionKind {
    /// A signature type with no navigable definition in this catalog.
    Type,
    Number(NumberValue),
    Reference(DeclarationTarget),
}

impl Region {
    pub fn number(&self) -> Option<&NumberValue> {
        match &self.kind {
            RegionKind::Number(value) => Some(value),
            _ => None,
        }
    }

    pub fn reference(&self) -> Option<EntityId> {
        match &self.kind {
            RegionKind::Reference(target) => Some(target.id),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct NumberValue {
    pub value: i128,
    pub bits: Option<u32>,
    pub unit: Option<&'static str>,
    pub entity: EntityId,
}

impl NumberValue {
    pub fn hexadecimal(&self) -> String {
        if let Some(bits) = self.bits {
            let value = self.value & ((1i128 << bits) - 1);
            format!("0x{value:0width$X}", width = (bits / 4) as usize)
        } else {
            format!("0x{:X}", self.value)
        }
    }

    pub fn with_unit(&self) -> String {
        self.unit.map_or_else(
            || self.value.to_string(),
            |unit| format!("{} {unit}", self.value),
        )
    }
}

#[derive(Default)]
pub struct Rendered {
    pub text: String,
    pub regions: Vec<Region>,
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
    fn context<T>(
        &mut self,
        pad: &str,
        arch: Architecture,
        label: &str,
        result: Result<T>,
        summary: impl FnOnce(&mut Self, &T) -> std::fmt::Result,
    ) -> Result<Option<T>> {
        write!(self, "{pad}// [{}] ", arch.name())?;
        let value = match result {
            Ok(value) => {
                summary(self, &value)?;
                Some(value)
            }
            Err(error) => {
                write!(self, "{label} unavailable: {error:#}")?;
                None
            }
        };
        writeln!(self)?;
        Ok(value)
    }

    fn fragment(&mut self, mut fragment: Self) {
        for region in &mut fragment.regions {
            region.range = region.range.start + self.position..region.range.end + self.position;
        }
        self.regions.extend(fragment.regions);
        self.write_str(&fragment.text).unwrap();
    }

    fn prefix(self, text: &str) -> Self {
        let mut out = Self::from(text);
        out.fragment(self);
        out
    }

    fn suffix(mut self, text: &str) -> Self {
        self.write_str(text).unwrap();
        self
    }

    fn join(parts: impl IntoIterator<Item = Self>, separator: &str) -> Self {
        let mut out = Self::default();
        for (index, part) in parts.into_iter().enumerate() {
            if index != 0 {
                out.write_str(separator).unwrap();
            }
            out.fragment(part);
        }
        out
    }

    fn number(
        &mut self,
        label: &str,
        value: u64,
        unit: Option<&'static str>,
        entity: EntityId,
    ) -> std::fmt::Result {
        let suffix = unit.map_or(String::new(), |unit| format!(" {unit}"));
        self.annotated(
            value.into(),
            None,
            unit,
            entity,
            format_args!("{label}{value}{suffix}"),
        )
    }

    fn annotated(
        &mut self,
        value: i128,
        bits: Option<u32>,
        unit: Option<&'static str>,
        entity: EntityId,
        text: std::fmt::Arguments<'_>,
    ) -> std::fmt::Result {
        let start = self.position;
        self.write_fmt(text)?;
        self.regions.push(Region {
            range: start..self.position,
            kind: RegionKind::Number(NumberValue {
                value,
                bits,
                unit,
                entity,
            }),
        });
        Ok(())
    }

    fn bit_position(&mut self, label: &str, bits: u64, entity: EntityId) -> std::fmt::Result {
        if bits.is_multiple_of(8) {
            self.number(label, bits / 8, Some("bytes"), entity)
        } else {
            self.number(label, bits, Some("bits"), entity)
        }
    }

    fn slot_position(
        &mut self,
        index: usize,
        arch: Architecture,
        entity: EntityId,
    ) -> std::fmt::Result {
        self.number("slot ", index as u64, None, entity)?;
        write!(self, ", ")?;
        let value = index as i128 * arch.pointer_bytes() as i128;
        self.annotated(
            value,
            None,
            Some("bytes"),
            entity,
            format_args!("vtable +0x{value:X}"),
        )
    }
}

impl From<&str> for Rendered {
    fn from(text: &str) -> Self {
        let mut out = Self::default();
        out.write_str(text).unwrap();
        out
    }
}

impl From<String> for Rendered {
    fn from(text: String) -> Self {
        Self::from(text.as_str())
    }
}

fn constant(
    out: &mut Rendered,
    value: &Value,
    t: &Type,
    arch: Architecture,
    entity: EntityId,
    suffix: &str,
) -> Result<()> {
    if let Some((value, bits)) = normalized_integer(value, t, arch) {
        out.annotated(value, Some(bits), None, entity, format_args!("{value}"))?;
        let hex = out.regions.last().unwrap().number().unwrap().hexadecimal();
        write!(out, "{suffix} // ")?;
        out.annotated(value, Some(bits), None, entity, format_args!("{hex}"))?;
    } else if let Value::Utf8(text) | Value::Utf16(text) = value {
        write!(out, "\"")?;
        for ch in text.chars() {
            match ch {
                '"' => write!(out, "\\\"")?,
                '\\' => write!(out, "\\\\")?,
                '\n' => write!(out, "\\n")?,
                '\r' => write!(out, "\\r")?,
                '\t' => write!(out, "\\t")?,
                ch if ch.is_control() => write!(out, "\\u{:04X}", ch as u32)?,
                ch => write!(out, "{ch}")?,
            }
        }
        write!(out, "\"{suffix} // ")?;
        out.number(
            "length: ",
            text.encode_utf16().count() as u64,
            Some("UTF-16 code units"),
            entity,
        )?;
    } else {
        let value = integer(value).map_or_else(|| format!("{value:?}"), |value| value.to_string());
        write!(out, "{value}{suffix}")?;
    }
    writeln!(out)?;
    Ok(())
}

pub fn render(catalog: &Catalog, id: EntityId, arch: Architecture) -> Result<Rendered> {
    render_target(catalog, &id.into(), arch)
}

pub fn render_target(
    catalog: &Catalog,
    target: &DeclarationTarget,
    arch: Architecture,
) -> Result<Rendered> {
    let target = catalog.bind_target(target, arch)?;
    let mut out = Rendered::default();
    append(&mut out, catalog, &target, arch, "")?;
    Ok(out)
}

/// Presentation names keep the definition ID; arguments are separate query context.
pub fn target_summary(
    catalog: &Catalog,
    target: &DeclarationTarget,
    arch: Architecture,
) -> Result<EntitySummary> {
    let mut summary = catalog.entity(target.id)?;
    if target.arguments.is_empty() {
        return Ok(summary);
    }
    let arguments = |qualified| {
        let types = Types {
            catalog,
            owner: target.id,
            arch,
            qualified,
        };
        Rendered::join(
            target.arguments.iter().map(|ty| types.ty(ty, "".into())),
            ", ",
        )
        .text
    };
    let instantiate = |name: &str, qualified| {
        format!(
            "{}<{}>",
            name.split('`').next().unwrap_or(name),
            arguments(qualified)
        )
    };
    match catalog.declaration(target.id, Some(arch))? {
        Declaration::Type(_) => {
            let prefix = summary.full_name.strip_suffix(&summary.name).unwrap_or("");
            summary.full_name = format!("{prefix}{}", instantiate(&summary.name, true));
            summary.name = instantiate(&summary.name, false);
        }
        row => {
            let Some(owner) = row.owner_id() else {
                return Ok(summary);
            };
            let owner = catalog.entity(owner)?;
            let prefix = owner.full_name.strip_suffix(&owner.name).unwrap_or("");
            summary.full_name = format!(
                "{prefix}{}.{}",
                instantiate(&owner.name, true),
                summary.name
            );
        }
    }
    Ok(summary)
}

fn generic_name<'a>(name: &str, params: impl Iterator<Item = GenericParam<'a>>) -> String {
    let mut params: Vec<_> = params.collect();
    params.sort_by_key(|p| p.sequence());
    if params.is_empty() {
        name.to_owned()
    } else {
        let names = params
            .iter()
            .map(|p| p.name())
            .collect::<Vec<_>>()
            .join(", ");
        format!("{}<{names}>", name.split('`').next().unwrap_or(name))
    }
}

fn method(
    out: &mut Rendered,
    types: &Types<'_>,
    m: MethodDef<'_>,
    target: &DeclarationTarget,
    name: Rendered,
    modifiers: &str,
    pad: &str,
) -> Result<()> {
    let signature = target.signature(m.signature(&[]))?;
    let params = m.params_by_sequence(signature.types.len())?;
    let mut parameters = signature
        .types
        .iter()
        .zip(params.params())
        .enumerate()
        .map(|(i, (t, p))| {
            let name = p
                .map(|p| p.name().to_owned())
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| format!("arg{}", i + 1));
            let flags = p.map_or_else(Vec::new, |p| {
                [
                    (p.flags().contains(ParamAttributes::In), "in"),
                    (p.flags().contains(ParamAttributes::Out), "out"),
                    (p.is_optional(), "optional"),
                    (p.is_reserved(), "reserved"),
                    (p.is_retval_attribute(), "retval"),
                    (!const_annotation(p).is_empty(), "const"),
                ]
                .into_iter()
                .filter_map(|(present, label)| present.then_some(label))
                .collect()
            });
            let prefix = if flags.is_empty() {
                String::new()
            } else {
                format!("[{}] ", flags.join(", "))
            };
            types.ty(t, name.into()).prefix(&prefix)
        })
        .collect::<Vec<_>>();
    if signature.flags.0 & 0x0f == MethodCallAttributes::VARARG.0 {
        parameters.push("...".into());
    }
    let parameters = if parameters.is_empty() {
        "void".into()
    } else if signature.types.is_empty() {
        "...".into()
    } else {
        Rendered::join(parameters, &format!(",\n{pad}    "))
            .prefix(&format!("\n{pad}    "))
            .suffix(&format!("\n{pad}"))
    };
    write!(out, "{pad}{modifiers}")?;
    write!(
        out,
        "{}",
        params
            .return_param()
            .map(const_annotation)
            .unwrap_or_default()
    )?;
    let mut name = name.suffix("(");
    name.fragment(parameters);
    name = name.suffix(")");
    out.fragment(types.ty(&signature.return_type, name));
    writeln!(out, ";")?;
    Ok(())
}

fn slot_comments(
    out: &mut Rendered,
    vtable: &winlex_core::Vtable,
    member: EntityId,
    arch: Architecture,
    pad: &str,
) -> std::fmt::Result {
    for (index, slot) in vtable
        .slots
        .iter()
        .enumerate()
        .filter(|(_, slot)| slot.id == Some(member))
    {
        write!(out, "{pad}// ")?;
        out.slot_position(index, arch, member)?;
        writeln!(out)?;
        if let Some(result) = &slot.abi {
            if let Ok(abi) = result {
                writeln!(
                    out,
                    "{pad}// scalar ABI ({}): {}({})",
                    abi.calling_convention,
                    abi.return_type,
                    abi.parameters
                        .iter()
                        .map(|p| format!("[{}] {} {}", p.direction, p.ty, p.name))
                        .collect::<Vec<_>>()
                        .join(", ")
                )?;
            } else if let Err(reason) = result {
                writeln!(out, "{pad}// ABI unsupported: {reason}")?;
            }
        }
    }
    Ok(())
}

fn append(
    out: &mut Rendered,
    c: &Catalog,
    target: &DeclarationTarget,
    arch: Architecture,
    pad: &str,
) -> Result<()> {
    let id = target.id;
    let row = c.declaration(id, Some(arch))?;
    let types = Types {
        catalog: c,
        owner: id,
        arch,
        qualified: false,
    };
    let is_function = row.category() == EntityCategory::Function;
    match row {
        Declaration::Type(t) => {
            let category = (t.name() != "<Module>").then(|| t.category());
            let layout = if category == Some(TypeCategory::Struct) {
                out.context(pad, arch, "Layout", c.layout(id, arch), |out, layout| {
                    out.bit_position("size: ", layout.size_bits, id)?;
                    write!(out, ", ")?;
                    out.bit_position("alignment: ", layout.alignment_bits, id)
                })?
            } else {
                None
            };
            let vtable = if category == Some(TypeCategory::Interface)
                || (category == Some(TypeCategory::Delegate)
                    && t.flags().contains(TypeAttributes::WindowsRuntime))
            {
                out.context(
                    pad,
                    arch,
                    "Vtable",
                    c.vtable_target(target, arch),
                    |out, vtable| {
                        out.number("vtable: ", vtable.slots.len() as u64, Some("slots"), id)
                    },
                )?
            } else {
                None
            };
            if target.has_instance_identity(t) {
                out.context(
                    pad,
                    arch,
                    "Instance IID",
                    c.interface_identity(target, arch),
                    |out, identity| write!(out, "IID: {}", identity.iid),
                )?;
            } else if let Some(guid) = guid(t) {
                writeln!(out, "{pad}// GUID: {guid}")?;
            }
            let keyword = match category {
                Some(TypeCategory::Interface) => "interface",
                Some(TypeCategory::Enum) => "enum",
                Some(TypeCategory::Class) => "class",
                Some(TypeCategory::Delegate) => "delegate",
                _ if t.flags().contains(TypeAttributes::ExplicitLayout) => {
                    "struct /* explicit layout */"
                }
                _ => "struct",
            };
            let name = if target.arguments.is_empty() {
                generic_name(t.name(), t.generic_params())
            } else {
                target_summary(c, target, arch)?.name
            };
            if category == Some(TypeCategory::Delegate)
                && let Some(invoke) = t.methods().find(|m| m.name() == "Invoke")
            {
                if let Some(vtable) = &vtable {
                    slot_comments(
                        out,
                        vtable,
                        winlex_core::metadata::key(invoke, 6),
                        arch,
                        pad,
                    )?;
                }
                write!(out, "{pad}delegate ")?;
                method(out, &types, invoke, target, name.into(), "", "")?;
                return Ok(());
            }
            let generics = t.generic_types();
            let mut bases = Vec::new();
            if let Some(base) = t.extends() {
                bases.push(types.ty(&target.ty(&base.ty(&generics))?, "".into()));
            }
            for interface in t.interface_impls() {
                bases.push(types.ty(&target.ty(&interface.interface(&generics))?, "".into()));
            }
            if category == Some(TypeCategory::Enum) {
                bases = t
                    .underlying_type()
                    .map(|t| types.ty(&t, "".into()))
                    .into_iter()
                    .collect();
            }
            let bases = if bases.is_empty() {
                Rendered::default()
            } else {
                Rendered::join(bases, ", ").prefix(" : ")
            };
            write!(out, "{pad}{keyword} {name}")?;
            out.fragment(bases);
            writeln!(out, "\n{pad}{{")?;
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
            if category == Some(TypeCategory::Enum) {
                for f in t.fields().filter(|f| f.constant().is_some()) {
                    if winlex_core::metadata::supports(f, arch.bit()) {
                        let value = f.constant().unwrap().value();
                        write!(out, "{inner}{} = ", f.name())?;
                        constant(
                            out,
                            &value,
                            &t.underlying_type().unwrap_or_else(|| value.ty()),
                            arch,
                            winlex_core::metadata::key(f, 4),
                            ",",
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
                    write!(out, "{inner}// ")?;
                    out.bit_position("offset: ", field.offset_bits, member)?;
                    write!(out, ", ")?;
                    out.bit_position("size: ", field.size_bits, member)?;
                    writeln!(out)?;
                    for bitfield in &field.bitfields {
                        write!(out, "{inner}// {}: ", bitfield.name)?;
                        let offset = field.offset_bits + bitfield.relative_offset_bits;
                        out.annotated(
                            offset.into(),
                            None,
                            Some("bits"),
                            member,
                            format_args!("bit offset {offset} bits from struct start"),
                        )?;
                        write!(out, ", ")?;
                        out.number("width ", bitfield.width_bits, Some("bits"), member)?;
                        writeln!(out)?;
                    }
                }
                if let Some(vtable) = &vtable {
                    slot_comments(out, vtable, member, arch, &inner)?;
                }
                append(out, c, &target.with_id(member), arch, &inner)?;
            }
            writeln!(out, "{pad}}};")?;
        }
        Declaration::Field(f) => {
            if let Some(offset) = f.explicit_offset() {
                write!(out, "{pad}// ")?;
                out.number("Declared offset: ", offset.into(), Some("bytes"), id)?;
                writeln!(out)?;
            }
            write!(out, "{pad}{}", const_annotation(f))?;
            out.fragment(types.ty(&target.ty(&f.ty())?, f.name().into()));
            if let Some(value) = f.constant() {
                // ELEMENT_TYPE_CLASS represents a null reference constant.
                if value.usize(0) == 0x12 {
                    writeln!(out, " = null;")?;
                    return Ok(());
                }
                let value = value.value();
                write!(out, " = ")?;
                let parent: TypeDef<'_> = f.parent_row(4);
                let t = if parent.category() == TypeCategory::Enum {
                    parent.underlying_type().unwrap_or_else(|| f.ty())
                } else {
                    f.ty()
                };
                constant(out, &value, &t, arch, id, ";")?;
                return Ok(());
            }
            writeln!(out, ";")?;
        }
        Declaration::Method(m) => {
            if let Some(import) = m.impl_map() {
                write!(out, "{pad}// ")?;
                let module = import.import_scope().name();
                if !module.is_empty() {
                    write!(out, "{module}!")?;
                }
                write!(out, "{}", import.import_name())?;
                let convention = m.calling_convention();
                if !convention.is_empty() {
                    write!(out, "; {convention}")?;
                }
                writeln!(out)?;
            }
            let name = generic_name(m.name(), m.generic_params());
            let name = if is_function || !pad.is_empty() {
                Rendered::from(name)
            } else {
                let owner = m.parent_row::<TypeDef>(5);
                let mut owner_type = TypeName::named(owner.namespace(), owner.name());
                let mut params: Vec<_> = owner.generic_params().collect();
                params.sort_by_key(|param| param.sequence());
                owner_type.generics = params
                    .iter()
                    .map(|param| Type::Generic(param.name().to_owned(), param.sequence()))
                    .collect();
                types.named(&owner_type).suffix(&format!("::{name}"))
            };
            let modifiers = if !is_function && m.flags().contains(MethodAttributes::Static) {
                "static "
            } else {
                ""
            };
            method(out, &types, m, target, name, modifiers, pad)?;
        }
        Declaration::Property(p) => {
            write!(out, "{pad}")?;
            out.fragment(types.ty(&target.ty(&p.signature().return_type)?, p.name().into()));
            writeln!(out, "; // property")?;
        }
        Declaration::Event(e) => {
            write!(out, "{pad}")?;
            out.fragment(types.ty(&target.ty(&e.ty())?, e.name().into()));
            writeln!(out, "; // event")?;
        }
        Declaration::Reference(_) => {
            writeln!(
                out,
                "{pad}// Type reference: {}",
                c.summarize(id, &row).full_name
            )?;
            match c.resolve_reference(id, arch) {
                Ok(target) => {
                    write!(out, "{pad}// Resolved: ")?;
                    let start = out.position;
                    write!(out, "{}", c.entity(target)?.full_name)?;
                    out.regions.push(Region {
                        range: start..out.position,
                        kind: RegionKind::Reference(target.into()),
                    });
                    writeln!(out)?;
                }
                Err(error) => writeln!(out, "{pad}// {error}")?,
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    include!("../tests/support/source.rs");

    #[test]
    fn pointer_qualifiers_preserve_c_levels_and_named_type_links() {
        use winlex_core::windows_metadata::writer;
        let mut file = writer::File::new("PointerQualifiers");
        file.TypeDef(
            "Fixture",
            "Record😀",
            writer::TypeDefOrRef::default(),
            TypeAttributes::Public,
        );
        let catalog =
            Catalog::from_inputs([source("pointer-qualifiers", file.into_stream())]).unwrap();
        let owner = catalog
            .type_ids()
            .find(|&id| catalog.entity(id).unwrap().name == "Record😀")
            .unwrap();
        let types = Types {
            catalog: &catalog,
            owner,
            arch: Architecture::X64,
            qualified: false,
        };
        let t = Type::value_named("Fixture", "Record😀");
        for (ty, expected) in [
            (Type::PtrMut(Box::new(t.clone()), 1), "Record😀 *value"),
            (
                Type::PtrConst(Box::new(t.clone()), 1),
                "const Record😀 *value",
            ),
            // C source contracts: const T **, T *const *, const T *const *.
            (
                Type::PtrMut(Box::new(Type::PtrConst(Box::new(t.clone()), 1)), 1),
                "const Record😀 **value",
            ),
            (
                Type::PtrConst(Box::new(Type::PtrMut(Box::new(t.clone()), 1)), 1),
                "Record😀 *const *value",
            ),
            (
                Type::PtrConst(Box::new(t.clone()), 2),
                "const Record😀 *const *value",
            ),
            (
                Type::PtrConst(Box::new(Type::ArrayFixed(Box::new(t.clone()), 3)), 1),
                "const Record😀 (*value)[3]",
            ),
            (
                Type::ArrayFixed(Box::new(Type::PtrConst(Box::new(t.clone()), 1)), 3),
                "const Record😀 *value[3]",
            ),
            (Type::RefMut(Box::new(t.clone())), "Record😀 &value"),
            (Type::RefConst(Box::new(t)), "const Record😀 value"),
        ] {
            let mut rendered = Rendered::from("// 文😀\r\n");
            rendered.fragment(types.ty(&ty, "value".into()));
            assert_eq!(rendered.text, format!("// 文😀\n{expected}"));
            let link = rendered
                .regions
                .iter()
                .find(|r| r.reference().is_some())
                .unwrap();
            assert_eq!(link.reference(), Some(owner));
            let units: Vec<_> = rendered.text.encode_utf16().collect();
            assert_eq!(
                String::from_utf16(&units[link.range.clone()]).unwrap(),
                "Record😀"
            );
        }
    }

    #[test]
    fn type_dispatch_preserves_module_attribute_and_explicit_layout_fallbacks() {
        use winlex_core::windows_metadata::writer;

        let mut file = writer::File::new("TypeDispatch");
        let cases = [
            (
                "Attribute",
                "OrdinaryAttribute",
                false,
                "struct OrdinaryAttribute : Attribute\n{\n};\n",
            ),
            (
                "Attribute",
                "ExplicitAttribute",
                true,
                "struct /* explicit layout */ ExplicitAttribute : Attribute\n{\n};\n",
            ),
            (
                "Object",
                "ExplicitClass",
                true,
                "class ExplicitClass : Object\n{\n};\n",
            ),
            ("", "<Module>", false, "struct <Module>\n{\n};\n"),
            (
                "ValueType",
                "<Module>",
                false,
                "struct <Module> : ValueType\n{\n};\n",
            ),
            ("Enum", "<Module>", false, "struct <Module> : Enum\n{\n};\n"),
            (
                "MulticastDelegate",
                "<Module>",
                false,
                "struct <Module> : MulticastDelegate\n{\n};\n",
            ),
            (
                "Attribute",
                "<Module>",
                true,
                "struct /* explicit layout */ <Module> : Attribute\n{\n};\n",
            ),
        ];
        for (base, name, explicit, _) in cases {
            let base = if base.is_empty() {
                writer::TypeDefOrRef::default()
            } else {
                writer::TypeDefOrRef::TypeRef(file.TypeRef("System", base))
            };
            file.TypeDef(
                "C2Kinds",
                name,
                base,
                TypeAttributes::Public
                    | if explicit {
                        TypeAttributes::ExplicitLayout
                    } else {
                        TypeAttributes::SequentialLayout
                    },
            );
        }
        let catalog =
            Catalog::from_inputs([source("winlex-type-dispatch", file.into_stream())]).unwrap();
        let ids: Vec<_> = catalog
            .type_ids()
            .filter(|&id| {
                let Declaration::Type(t) = catalog.declaration(id, None).unwrap() else {
                    unreachable!()
                };
                t.namespace() == "C2Kinds"
            })
            .collect();
        assert_eq!(ids.len(), cases.len());
        for (id, (base, _, _, expected)) in ids.into_iter().zip(cases) {
            for arch in Architecture::ALL {
                let rendered = render(&catalog, id, arch).unwrap();
                assert_eq!(rendered.text, expected);
                let units: Vec<_> = rendered.text.encode_utf16().collect();
                let types: Vec<_> = rendered
                    .regions
                    .iter()
                    .map(|region| {
                        assert!(matches!(region.kind, RegionKind::Type));
                        String::from_utf16(&units[region.range.clone()]).unwrap()
                    })
                    .collect();
                assert_eq!(types, if base.is_empty() { vec![] } else { vec![base] });
            }
        }
    }

    #[test]
    fn labeled_positions_capture_complete_regions_without_separators() {
        let mut out = Rendered::default();
        write!(out, "😀\r\n").unwrap();
        out.bit_position("offset: ", 24, entity()).unwrap();
        write!(out, ", ").unwrap();
        out.bit_position("size: ", 7, entity()).unwrap();
        write!(out, "; ").unwrap();
        out.number("vtable: ", 3, Some("slots"), entity()).unwrap();
        let units: Vec<_> = out.text.encode_utf16().collect();
        assert_eq!(
            out.text,
            "😀\noffset: 3 bytes, size: 7 bits; vtable: 3 slots"
        );
        for (region, (text, value, unit)) in out.regions.iter().zip([
            ("offset: 3 bytes", 3, "bytes"),
            ("size: 7 bits", 7, "bits"),
            ("vtable: 3 slots", 3, "slots"),
        ]) {
            assert_eq!(
                String::from_utf16(&units[region.range.clone()]).unwrap(),
                text
            );
            let number = region.number().unwrap();
            assert_eq!(number.value, value);
            assert_eq!(number.unit, Some(unit));
            assert_eq!(number.entity, entity());
        }
        assert_eq!(out.regions.len(), 3);
        assert_eq!(out.regions[0].range.start, 3);
    }

    fn entity() -> EntityId {
        EntityId {
            source_id: 0,
            token: 0x04000001,
        }
    }

    fn render_constant(value: &Value, t: &Type, arch: Architecture, suffix: &str) -> Rendered {
        let mut out = Rendered::default();
        constant(&mut out, value, t, arch, entity(), suffix).unwrap();
        out
    }

    #[test]
    fn integer_annotations_preserve_signedness_and_width() {
        let cases = [
            (Value::I8(i8::MIN), "-128", "0x80"),
            (Value::U8(u8::MAX), "255", "0xFF"),
            (Value::I16(i16::MIN), "-32768", "0x8000"),
            (Value::U16(u16::MAX), "65535", "0xFFFF"),
            (Value::I32(-1073741819), "-1073741819", "0xC0000005"),
            (Value::U32(u32::MAX), "4294967295", "0xFFFFFFFF"),
            (
                Value::I64(i64::MIN),
                "-9223372036854775808",
                "0x8000000000000000",
            ),
            (
                Value::U64(u64::MAX),
                "18446744073709551615",
                "0xFFFFFFFFFFFFFFFF",
            ),
            (Value::U32(0), "0", "0x00000000"),
            (Value::I32(i32::MAX), "2147483647", "0x7FFFFFFF"),
        ];
        for (value, decimal, hex) in cases {
            let out = render_constant(&value, &value.ty(), Architecture::X64, ";");
            assert_eq!(out.text, format!("{decimal}; // {hex}\n"));
            let utf16: Vec<_> = out.text.encode_utf16().collect();
            for (region, expected) in out.regions.iter().zip([decimal, hex]) {
                assert_eq!(
                    String::from_utf16(&utf16[region.range.clone()]).unwrap(),
                    expected
                );
                assert_eq!(region.number().unwrap().value.to_string(), decimal);
                assert_eq!(region.number().unwrap().hexadecimal(), hex);
            }
        }
        let out = render_constant(&Value::U32(0xFFFF_FFFF), &Type::I32, Architecture::X64, ",");
        assert_eq!(out.text, "-1, // 0xFFFFFFFF\n");
    }

    #[test]
    fn strings_escape_content_and_count_original_utf16_units() {
        for text in ["", "Example", "中😀", "\"\\\r\n\t\0\u{1}A"] {
            let mut out = Rendered::default();
            write!(out, "// 文😀\r\n").unwrap();
            constant(
                &mut out,
                &Value::Utf16(text.into()),
                &Type::String,
                Architecture::X64,
                entity(),
                ";",
            )
            .unwrap();
            let region = &out.regions[0];
            let number = region.number().unwrap();
            assert_eq!(number.value, text.encode_utf16().count() as i128);
            assert_eq!(
                number.with_unit(),
                format!("{} UTF-16 code units", text.encode_utf16().count())
            );
            let units: Vec<_> = out.text.encode_utf16().collect();
            assert_eq!(
                String::from_utf16(&units[region.range.clone()]).unwrap(),
                format!("length: {}", number.with_unit())
            );
            assert_eq!(out.text.lines().count(), 2);
            assert!(!out.text.contains('\0'));
            if text.contains('\0') {
                assert!(out.text.contains("\\\"\\\\\\r\\n\\t\\u0000\\u0001A"));
            }
        }
    }

    #[test]
    fn pointer_sized_constants_follow_architecture_and_other_values_stay_unannotated() {
        for (arch, expected) in [
            (Architecture::X86, "-1; // 0xFFFFFFFF\n"),
            (Architecture::X64, "-1; // 0xFFFFFFFFFFFFFFFF\n"),
        ] {
            let out = render_constant(&Value::ISize(-1), &Type::ISize, arch, ";");
            assert_eq!(out.text, expected);
        }
        for (value, t, expected) in [
            (Value::Bool(true), Type::Bool, "Bool(true);\n"),
            (Value::F32(1.5), Type::F32, "F32(1.5);\n"),
            (Value::U16(65), Type::Char, "65;\n"),
        ] {
            let out = render_constant(&value, &t, Architecture::X64, ";");
            assert_eq!(out.text, expected);
            assert!(out.regions.is_empty());
        }
    }

    #[test]
    fn vtable_context_regions_cover_labels_but_not_separators() {
        for (arch, hex) in [(Architecture::X86, "0xC"), (Architecture::X64, "0x18")] {
            let mut out = Rendered::default();
            out.slot_position(3, arch, entity()).unwrap();
            assert_eq!(out.text, format!("slot 3, vtable +{hex}"));
            let utf16: Vec<_> = out.text.encode_utf16().collect();
            for (region, expected) in out
                .regions
                .iter()
                .zip(["slot 3".to_owned(), format!("vtable +{hex}")])
            {
                assert_eq!(
                    String::from_utf16(&utf16[region.range.clone()]).unwrap(),
                    expected
                );
                for index in region.range.clone() {
                    assert_eq!(
                        out.regions
                            .iter()
                            .filter(|n| n.range.contains(&index))
                            .count(),
                        1
                    );
                }
            }
            assert!(
                !out.regions
                    .iter()
                    .any(|n| n.range.contains(&6) || n.range.contains(&7))
            );
        }
    }

    #[test]
    fn composed_references_follow_utf16_and_normalized_newlines() {
        let mut fragment = Rendered::from("😀Target");
        fragment.regions.push(Region {
            range: 2..8,
            kind: RegionKind::Reference(entity().into()),
        });
        let mut out = Rendered::from("// 文😀\r\n");
        out.fragment(fragment.prefix("const ").suffix(" *value;\r\n"));
        assert_eq!(out.text, "// 文😀\nconst 😀Target *value;\n");
        assert_eq!(out.regions[0].range, 15..21);
        assert_eq!(out.regions[0].reference(), Some(entity()));
        let units: Vec<_> = out.text.encode_utf16().collect();
        assert_eq!(
            String::from_utf16(&units[out.regions[0].range.clone()]).unwrap(),
            "Target"
        );
    }
}
