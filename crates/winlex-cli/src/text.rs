//! Text presentation reads typed results and borrowed metadata directly.
use super::{Page, Query};
use std::fmt::Write;
use winlex_core::windows_metadata::{reader::*, *};
use winlex_core::{
    Architecture, Catalog, Declaration, EntityId, Result, guid, integer, metadata::key,
};
use winlex_format::member_ids;

fn ty(t: &Type) -> String {
    match t {
        Type::ClassName(n) | Type::ValueName(n) => {
            let args = if n.generics.is_empty() {
                String::new()
            } else {
                format!(
                    "<{}>",
                    n.generics.iter().map(ty).collect::<Vec<_>>().join(", ")
                )
            };
            format!(
                "{}{args} -> {}",
                n.name,
                n.identity
                    .map(|(source_id, token)| EntityId { source_id, token }.to_string())
                    .unwrap_or_default()
            )
        }
        Type::Generic(n, i) => format!("{n} (type parameter {i})"),
        Type::MethodGeneric(n, i) => format!("{n} (method parameter {i})"),
        Type::PtrMut(t, d) => format!("{}{}", ty(t), "*".repeat((*d).min(256))),
        Type::PtrConst(t, d) => format!("const {}{}", ty(t), "*".repeat((*d).min(256))),
        Type::ArrayFixed(t, n) => format!("{}[{n}]", ty(t)),
        Type::Array(t) => format!("{}[]", ty(t)),
        Type::RefMut(t) => format!("{}&", ty(t)),
        Type::RefConst(t) => format!("const {}", ty(t)),
        t => format!("{t:?}"),
    }
}
fn value(v: &Value) -> String {
    if let Value::EnumValue(n, v) = v {
        return format!("{}.{}({})", n.namespace, n.name, value(v));
    }
    if let Some(n) = integer(v) {
        let bits = match v {
            Value::U8(_) | Value::I8(_) => 8,
            Value::U16(_) | Value::I16(_) => 16,
            Value::U32(_) | Value::I32(_) => 32,
            _ => 64,
        };
        return format!(
            "{n} (0x{:0width$x}, {:?})",
            n & ((1i128 << bits) - 1),
            v.ty(),
            width = bits / 4
        );
    }
    format!("{v:?}")
}
fn attributes<'a>(out: &mut String, pad: &str, r: impl HasAttributes<'a>) -> Result<()> {
    for a in r.attributes() {
        let args = a
            .value()
            .iter()
            .map(|(n, v)| format!("{n}={}", value(v)))
            .collect::<Vec<_>>()
            .join(", ");
        writeln!(out, "{pad}  @{} ({args})", a.name())?;
    }
    Ok(())
}
fn accessors<'a>(
    out: &mut String,
    pad: &str,
    rows: impl Iterator<Item = MethodSemantics<'a>>,
) -> Result<()> {
    for a in rows {
        writeln!(
            out,
            "{pad}  accessor {}: {}",
            a.method().name(),
            key(a.method(), 6)
        )?;
    }
    Ok(())
}
fn bits(out: &mut String, pad: &str, fields: &[winlex_core::Bitfield], offset: u64) -> Result<()> {
    for b in fields {
        writeln!(
            out,
            "{pad}  bitfield {}: offset={} bits, width={} bits",
            b.name,
            offset + b.relative_offset_bits,
            b.width_bits
        )?;
    }
    Ok(())
}
fn declaration(
    out: &mut String,
    c: &Catalog,
    id: EntityId,
    arch: Option<Architecture>,
    depth: usize,
) -> Result<()> {
    let row = c.declaration(id, arch)?;
    let e = c.summarize(id, &row);
    let pad = "  ".repeat(depth);
    writeln!(out, "{pad}{} {} {}", e.id, e.kind, e.full_name)?;
    match row {
        Declaration::Type(t) => {
            if let Some(g) = guid(t) {
                writeln!(out, "{pad}  GUID: {g}")?;
            }
            let kind = if t.flags().contains(TypeAttributes::ExplicitLayout) {
                "explicit"
            } else if t.flags().contains(TypeAttributes::SequentialLayout) {
                "sequential"
            } else {
                "auto"
            };
            write!(out, "{pad}  layout: {kind}")?;
            if let Some(l) = t.class_layout() {
                write!(
                    out,
                    " pack={} bytes, declared size={} bytes",
                    l.packing_size(),
                    l.class_size()
                )?;
            }
            writeln!(out)?;
            if let Some(b) = t.extends() {
                let base = match b {
                    TypeDefOrRef::TypeSpec(s) => s.ty(&t.generic_types()),
                    b => Type::ClassName(b.type_name()),
                };
                writeln!(out, "{pad}  extends: {}", ty(&base))?;
            }
            attributes(out, &pad, t)?;
            for i in t.interface_impls() {
                writeln!(
                    out,
                    "{pad}  interface: {}{}",
                    ty(&i.interface(&t.generic_types())),
                    if i.has_attribute("DefaultAttribute") {
                        " [default]"
                    } else {
                        ""
                    }
                )?;
            }
            let ids = member_ids(t.fields(), 4, arch)
                .chain(member_ids(t.properties(), 0x17, arch))
                .chain(member_ids(t.events(), 0x14, arch))
                .chain(member_ids(t.methods(), 6, arch));
            for id in ids {
                declaration(out, c, id, arch, depth + 1)?;
            }
        }
        Declaration::Field(f) => {
            bits(out, &pad, &winlex_core::metadata::bitfields(f)?, 0)?;
            writeln!(out, "{pad}  type: {}", ty(&f.ty()))?;
            if let Some(offset) = f.explicit_offset() {
                writeln!(out, "{pad}  declared offset: {offset} bytes")?;
            }
            if let Some(v) = f.constant() {
                writeln!(out, "{pad}  value: {}", value(&v.value()))?;
            }
            if let Some(g) = guid(f) {
                writeln!(out, "{pad}  GUID: {g}")?;
            }
            attributes(out, &pad, f)?;
        }
        Declaration::Method(m) => {
            let s = m.signature(&[]);
            let ps = m.params_by_sequence(s.types.len())?;
            writeln!(out, "{pad}  returns: {}", ty(&s.return_type))?;
            if let Some(i) = m.impl_map() {
                writeln!(
                    out,
                    "{pad}  import: {}!{} flags={} calling-convention={:?}",
                    i.import_scope().name(),
                    i.import_name(),
                    i.flags().0,
                    m.calling_convention()
                )?;
            }
            for (i, (t, p)) in s.types.iter().zip(ps.params()).enumerate() {
                writeln!(
                    out,
                    "{pad}  [{}] {}: {} ({})",
                    i + 1,
                    p.map(|p| p.name().to_owned()).unwrap_or_default(),
                    ty(t),
                    p.map(|p| format!("{:?}", p.direction()))
                        .unwrap_or_default()
                )?;
            }
            attributes(out, &pad, m)?;
        }
        Declaration::Property(p) => {
            writeln!(out, "{pad}  type: {}", ty(&p.signature().return_type))?;
            accessors(out, &pad, p.accessors())?;
            attributes(out, &pad, p)?;
        }
        Declaration::Event(e) => {
            writeln!(out, "{pad}  type: {}", ty(&e.ty()))?;
            accessors(out, &pad, e.accessors())?;
            attributes(out, &pad, e)?;
        }
        Declaration::Reference(r) => {
            write!(out, "{pad}  scope: 0x{:08x}", r.scope_token())?;
            if let Some(a) = arch {
                match c.resolve_reference(id, a) {
                    Ok(id) => write!(out, "; resolution: {id}")?,
                    Err(e) => write!(out, "; unresolved: {e}")?,
                }
            }
            writeln!(out)?;
        }
    }
    Ok(())
}
fn page(out: &mut String, p: &Page) -> Result<()> {
    writeln!(
        out,
        "{} match(es); offset {}, limit {}",
        p.total, p.offset, p.limit
    )?;
    for e in &p.matches {
        writeln!(
            out,
            "{}  {:10} arch={}  {}",
            e.id, e.kind, e.architecture_mask, e.full_name
        )?;
    }
    Ok(())
}
fn fields(out: &mut String, rows: &[winlex_core::FieldLayout], depth: usize) -> Result<()> {
    let pad = "  ".repeat(depth);
    for f in rows {
        writeln!(
            out,
            "{pad}{} offset={} bits, size={} bits",
            f.name, f.offset_bits, f.size_bits
        )?;
        bits(out, &pad, &f.bitfields, f.offset_bits)?;
        fields(out, &f.fields, depth + 1)?;
    }
    Ok(())
}
pub(super) fn render(q: &Query, c: &Catalog, arch: Option<Architecture>) -> Result<String> {
    let mut out = String::new();
    match q {
        Query::Summary => {
            let (types, fields, methods) = c.row_counts();
            writeln!(
                out,
                "{types} type rows, {fields} field rows, {methods} method rows"
            )?;
            for s in c.sources() {
                writeln!(out, "{}: {}", s.id, s.path.display())?;
                if let Some(a) = &s.assembly {
                    writeln!(out, "  assembly: {a:?}")?;
                }
                for r in &s.references {
                    writeln!(out, "  reference: {r:?}")?;
                }
            }
        }
        Query::Matches(p) => page(&mut out, p)?,
        Query::References(id, p, unresolved) => {
            writeln!(out, "References to {id} (unresolved uses: {unresolved})")?;
            page(&mut out, p)?;
        }
        Query::Show(id) => declaration(&mut out, c, *id, arch, 0)?,
        Query::Layout(l) => {
            writeln!(
                out,
                "{} [{}] size={} bits, alignment={} bits",
                l.name,
                l.architecture.name(),
                l.size_bits,
                l.alignment_bits
            )?;
            fields(&mut out, &l.fields, 1)?;
        }
        Query::Vtable(v) => {
            writeln!(out, "{} [{}]", v.name, v.architecture.name())?;
            for (i, s) in v.slots.iter().enumerate() {
                writeln!(
                    out,
                    "{i:>3}  {}.{}  {}  byte-offset={}",
                    s.declaring_type,
                    s.method,
                    s.id.map(|id| id.to_string())
                        .unwrap_or_else(|| "[implicit ABI]".into()),
                    i * if v.architecture == Architecture::X86 {
                        4
                    } else {
                        8
                    }
                )?;
            }
        }
    }
    Ok(out)
}
