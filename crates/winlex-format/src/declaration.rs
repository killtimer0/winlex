//! Compact C/IDL-like reading view; not a compilable header or an ABI definition.
use super::integer_format;
use std::fmt::Write;
use winlex_core::windows_metadata::reader::*;
use winlex_core::windows_metadata::*;
use winlex_core::{Architecture, Catalog, Declaration, EntityId, Result, guid, integer};

struct Types<'a> {
    catalog: &'a Catalog,
    owner: EntityId,
    arch: Architecture,
}

impl Types<'_> {
    fn ty(&self, t: &Type, name: Rendered) -> Rendered {
        match t {
            Type::ArrayFixed(t, n) => self.ty(t, name.suffix(&format!("[{n}]"))),
            Type::Array(t) => self.ty(t, name.suffix("[]")),
            pointer @ (Type::PtrMut(t, depth) | Type::PtrConst(t, depth)) => {
                let name = name.prefix(&"*".repeat((*depth).min(256)));
                let name = if matches!(**t, Type::ArrayFixed(..) | Type::Array(..)) {
                    name.prefix("(").suffix(")")
                } else {
                    name
                };
                let rendered = self.ty(t, name);
                if matches!(pointer, Type::PtrConst(..)) {
                    rendered.prefix("const ")
                } else {
                    rendered
                }
            }
            Type::RefMut(t) => self.ty(t, name.prefix("&")),
            Type::RefConst(t) => self.ty(t, name).prefix("const "),
            _ => {
                let base = match t {
                    Type::ClassName(n) | Type::ValueName(n) => {
                        let head = if n.generics.is_empty() {
                            n.name.as_str()
                        } else {
                            n.name.split('`').next().unwrap_or(&n.name)
                        };
                        let mut base = Rendered::from(head);
                        if let Ok(target) = self.catalog.resolve_type(self.owner, n, self.arch) {
                            base.regions.push(Region {
                                range: 0..base.position,
                                kind: RegionKind::Reference(target),
                            });
                        }
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
                    Type::Generic(n, _) | Type::MethodGeneric(n, _) => n.to_string().into(),
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
                    other => format!("{other:?}").into(),
                };
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
}

/// A semantic region in RichEdit UTF-16 coordinates (one code unit per newline).
#[derive(Clone, Debug)]
pub struct Region {
    pub range: std::ops::Range<usize>,
    pub kind: RegionKind,
}

#[derive(Clone, Debug)]
pub enum RegionKind {
    Number(NumberValue),
    Reference(EntityId),
}

impl Region {
    pub fn number(&self) -> Option<&NumberValue> {
        match &self.kind {
            RegionKind::Number(value) => Some(value),
            _ => None,
        }
    }

    pub fn reference(&self) -> Option<EntityId> {
        match self.kind {
            RegionKind::Reference(id) => Some(id),
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
    if let Some((bits, signed)) = integer_format(t, arch).or_else(|| {
        matches!(t, Type::ValueName(_) | Type::ClassName(_))
            .then(|| integer_format(&value.ty(), arch))
            .flatten()
    }) && let Some(raw) = integer(value)
    {
        let mask = (1i128 << bits) - 1;
        let raw = raw & mask;
        let value = if signed && raw & (1i128 << (bits - 1)) != 0 {
            raw - (1i128 << bits)
        } else {
            raw
        };
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
    name: &str,
    pad: &str,
) -> Result<()> {
    let signature = m.signature(&[]);
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
                    (p.has_attribute("ConstAttribute"), "const"),
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
    write!(out, "{pad}")?;
    out.fragment(types.ty(
        &signature.return_type,
        parameters.prefix(&format!("{name}(")).suffix(")"),
    ));
    writeln!(out, ";")?;
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
    let types = Types {
        catalog: c,
        owner: id,
        arch,
    };
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
            let vtable = if category == Some(TypeCategory::Interface) {
                out.context(pad, arch, "Vtable", c.vtable(id, arch), |out, vtable| {
                    out.number("vtable: ", vtable.slots.len() as u64, Some("slots"), id)
                })?
            } else {
                None
            };
            if let Some(guid) = guid(t) {
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
            let name = generic_name(t.name(), t.generic_params());
            if category == Some(TypeCategory::Delegate)
                && let Some(invoke) = t.methods().find(|m| m.name() == "Invoke")
            {
                write!(out, "{pad}delegate ")?;
                method(out, &types, invoke, &name, "")?;
                return Ok(());
            }
            let generics = t.generic_types();
            let mut bases = Vec::new();
            if let Some(base) = t.extends() {
                bases.push(types.ty(&base.ty(&generics), "".into()));
            }
            bases.extend(
                t.interface_impls()
                    .map(|i| types.ty(&i.interface(&generics), "".into())),
            );
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
                write!(out, "{pad}// ")?;
                out.number("Declared offset: ", offset.into(), Some("bytes"), id)?;
                writeln!(out)?;
            }
            write!(out, "{pad}")?;
            out.fragment(types.ty(&f.ty(), f.name().into()));
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
            method(
                out,
                &types,
                m,
                &generic_name(m.name(), m.generic_params()),
                pad,
            )?;
        }
        Declaration::Property(p) => {
            write!(out, "{pad}")?;
            out.fragment(types.ty(&p.signature().return_type, p.name().into()));
            writeln!(out, "; // property")?;
        }
        Declaration::Event(e) => {
            write!(out, "{pad}")?;
            out.fragment(types.ty(&e.ty(), e.name().into()));
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
                        kind: RegionKind::Reference(target),
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

    #[test]
    fn type_dispatch_preserves_module_attribute_and_explicit_layout_fallbacks() {
        use winlex_core::{SourceData, windows_metadata::writer};

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
        let path =
            std::env::temp_dir().join(format!("winlex-type-dispatch-{}.winmd", std::process::id()));
        std::fs::write(&path, file.into_stream()).unwrap();
        let source = SourceData::read(&path).unwrap();
        std::fs::remove_file(path).unwrap();
        let catalog = Catalog::from_inputs([source]).unwrap();
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
        for (id, (_, _, _, expected)) in ids.into_iter().zip(cases) {
            for arch in Architecture::ALL {
                let rendered = render(&catalog, id, arch).unwrap();
                assert_eq!(rendered.text, expected);
                assert!(rendered.regions.is_empty());
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
            let mut out = Rendered::default();
            constant(
                &mut out,
                &value,
                &value.ty(),
                Architecture::X64,
                entity(),
                ";",
            )
            .unwrap();
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
        let mut out = Rendered::default();
        constant(
            &mut out,
            &Value::U32(0xFFFF_FFFF),
            &Type::I32,
            Architecture::X64,
            entity(),
            ",",
        )
        .unwrap();
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
            let mut out = Rendered::default();
            constant(
                &mut out,
                &Value::ISize(-1),
                &Type::ISize,
                arch,
                entity(),
                ";",
            )
            .unwrap();
            assert_eq!(out.text, expected);
        }
        for (value, t, expected) in [
            (Value::Bool(true), Type::Bool, "Bool(true);\n"),
            (Value::F32(1.5), Type::F32, "F32(1.5);\n"),
            (Value::U16(65), Type::Char, "65;\n"),
        ] {
            let mut out = Rendered::default();
            constant(&mut out, &value, &t, Architecture::X64, entity(), ";").unwrap();
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
            kind: RegionKind::Reference(entity()),
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
