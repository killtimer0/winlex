//! Text presentation reads typed results and borrowed metadata directly.
use super::{Page, Query};
use std::fmt::Write;
use winlex_core::windows_metadata::{reader::*, *};
use winlex_core::{
    Architecture, Catalog, Declaration, DeclarationTarget, EntityId, Result, guid, metadata::key,
};
use winlex_format::{attribute_value as value, const_annotation, layout_kind, member_ids};

fn ty(t: &Type) -> String {
    qualified_ty(t, false)
}

fn qualified_ty(t: &Type, is_const: bool) -> String {
    if let Type::PtrMut(inner, depth) | Type::PtrConst(inner, depth) = t {
        let pointee_const = matches!(t, Type::PtrConst(..));
        let mut out = qualified_ty(inner, pointee_const);
        let depth = (*depth).min(256);
        for level in 0..depth {
            if pointee_const
                && (level > 0 || matches!(**inner, Type::PtrMut(..) | Type::PtrConst(..)))
            {
                out.push(' ');
            }
            out.push('*');
            if if level + 1 == depth {
                is_const
            } else {
                pointee_const
            } {
                out.push_str("const");
            }
        }
        return out;
    }
    let rendered = match t {
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
        Type::ArrayFixed(t, n) => format!("{}[{n}]", ty(t)),
        Type::Array(t) => format!("{}[]", ty(t)),
        Type::RefMut(t) => format!("{}&", ty(t)),
        Type::RefConst(t) => format!("const {}", ty(t)),
        t => format!("{t:?}"),
    };
    if is_const {
        format!("const {rendered}")
    } else {
        rendered
    }
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
    target: &DeclarationTarget,
    arch: Option<Architecture>,
    depth: usize,
) -> Result<()> {
    let id = target.id;
    let row = c.declaration(id, arch)?;
    let e =
        winlex_format::declaration::target_summary(c, target, arch.unwrap_or(Architecture::X64))?;
    let pad = "  ".repeat(depth);
    writeln!(out, "{pad}{} {} {}", e.id, e.kind, e.full_name)?;
    match row {
        Declaration::Type(t) => {
            if let Some(g) = guid(t) {
                let label = if target.arguments.is_empty() {
                    "GUID"
                } else {
                    "definition GUID"
                };
                writeln!(out, "{pad}  {label}: {g}")?;
            }
            if target.has_instance_identity(t) {
                match c.interface_identity(target, arch.expect("instance architecture")) {
                    Ok(identity) => writeln!(out, "{pad}  IID: {}", identity.iid)?,
                    Err(error) => writeln!(out, "{pad}  IID unavailable: {error:#}")?,
                }
            }
            write!(out, "{pad}  layout: {}", layout_kind(t.flags()))?;
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
                writeln!(
                    out,
                    "{pad}  extends: {}",
                    ty(&target.ty(&b.ty(&t.generic_types()))?)
                )?;
            }
            attributes(out, &pad, t)?;
            for i in t.interface_impls() {
                writeln!(
                    out,
                    "{pad}  interface: {}{}",
                    ty(&target.ty(&i.interface(&t.generic_types()))?),
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
                declaration(out, c, &target.with_id(id), arch, depth + 1)?;
            }
        }
        Declaration::Field(f) => {
            bits(out, &pad, &winlex_core::metadata::bitfields(f)?, 0)?;
            writeln!(
                out,
                "{pad}  type: {}{}",
                const_annotation(f),
                ty(&target.ty(&f.ty())?)
            )?;
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
            let s = target.signature(m.signature(&[]))?;
            let ps = m.params_by_sequence(s.types.len())?;
            writeln!(
                out,
                "{pad}  returns: {}{}",
                ps.return_param().map(const_annotation).unwrap_or_default(),
                ty(&s.return_type)
            )?;
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
                    "{pad}  [{}] {}: {}{} ({})",
                    i + 1,
                    p.as_ref().map(|p| p.name()).unwrap_or_default(),
                    p.map(const_annotation).unwrap_or_default(),
                    ty(t),
                    p.map(|p| format!("{:?}", p.direction()))
                        .unwrap_or_default()
                )?;
            }
            attributes(out, &pad, m)?;
        }
        Declaration::Property(p) => {
            writeln!(
                out,
                "{pad}  type: {}",
                ty(&target.ty(&p.signature().return_type)?)
            )?;
            accessors(out, &pad, p.accessors())?;
            attributes(out, &pad, p)?;
        }
        Declaration::Event(e) => {
            writeln!(out, "{pad}  type: {}", ty(&target.ty(&e.ty())?))?;
            accessors(out, &pad, e.accessors())?;
            attributes(out, &pad, e)?;
        }
        Declaration::Reference(r) => {
            write!(out, "{pad}  scope: 0x{:08x}", r.scope_token())?;
            if let Some(foundation) = r.foundation_type() {
                write!(out, "; builtin: {foundation:?}; {}", foundation.semantics())?;
            } else if let Some(a) = arch {
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
        Query::Show(target) => declaration(&mut out, c, target, arch, 0)?,
        Query::Iid(identity) => {
            writeln!(
                out,
                "IID: {}\nSignature: {}",
                identity.iid, identity.signature
            )?;
        }
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
                    i * v.architecture.pointer_bytes()
                )?;
            }
            if let Some(instance) = &v.instance {
                writeln!(
                    out,
                    "IID: {}\nSignature: {}",
                    instance.iid, instance.signature
                )?;
                for (slot, method) in v.slots.iter().enumerate() {
                    if let Some(Ok(abi)) = &method.abi {
                        writeln!(
                            out,
                            "  slot {} scalar ABI ({}): {}({})",
                            slot,
                            abi.calling_convention,
                            abi.return_type,
                            abi.parameters
                                .iter()
                                .map(|p| format!("[{}] {} {}", p.direction, p.ty, p.name))
                                .collect::<Vec<_>>()
                                .join(", ")
                        )?;
                    } else if let Some(Err(reason)) = &method.abi {
                        writeln!(out, "  slot {slot} ABI unsupported: {reason}")?;
                    }
                }
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pointer_qualifiers_follow_the_c_pointee_level() {
        for (t, expected) in [
            (Type::PtrMut(Box::new(Type::I32), 1), "I32*"),
            (Type::PtrConst(Box::new(Type::I32), 1), "const I32*"),
            // const int **, int *const *, and const int *const * are distinct C types.
            (
                Type::PtrMut(Box::new(Type::PtrConst(Box::new(Type::I32), 1)), 1),
                "const I32**",
            ),
            (
                Type::PtrConst(Box::new(Type::PtrMut(Box::new(Type::I32), 1)), 1),
                "I32*const *",
            ),
            (Type::PtrConst(Box::new(Type::I32), 2), "const I32*const *"),
            (
                Type::ArrayFixed(Box::new(Type::PtrConst(Box::new(Type::I32), 1)), 3),
                "const I32*[3]",
            ),
            (Type::RefMut(Box::new(Type::I32)), "I32&"),
            (Type::RefConst(Box::new(Type::I32)), "const I32"),
        ] {
            assert_eq!(ty(&t), expected);
        }
    }
}
