//! Presentation adapters shared by the CLI and native declaration viewer.
pub mod declaration;
use serde_json::{Value as Json, json};
use winlex_core::metadata::key;
use winlex_core::windows_metadata::{reader::*, *};
use winlex_core::{
    Architecture, Catalog, Declaration, DeclarationTarget, EntityId, Result, guid, integer,
    primitive_name,
};

pub fn const_annotation<'a>(owner: impl HasAttributes<'a>) -> &'static str {
    if owner.attributes().any(|a| {
        a.namespace() == "Windows.Win32.Foundation.Metadata" && a.name() == "ConstAttribute"
    }) {
        "[const] "
    } else {
        ""
    }
}

pub fn constant(value: &Value) -> String {
    if let Some(value) = integer(value) {
        return value.to_string();
    }
    match value {
        Value::EnumValue(_, value) => constant(value),
        Value::Bool(value) => value.to_string(),
        Value::F32(value) => value.to_string(),
        Value::F64(value) => value.to_string(),
        Value::Utf8(value) | Value::Utf16(value) => format!("{value:?}"),
        _ => format!("{value:?}"),
    }
}

pub fn summary(c: &Catalog) -> Json {
    let (types, fields, methods) = c.row_counts();
    json!({
        "sources": c.sources(),
        "type_rows": types,
        "field_rows": fields,
        "method_rows": methods,
    })
}
fn integer_text(v: &Value) -> Option<(String, String, usize)> {
    let n = integer(v)?;
    let bits = integer_format(&v.ty(), Architecture::X64)?.0 as usize;
    Some((
        n.to_string(),
        format!("0x{:0width$x}", n & ((1i128 << bits) - 1), width = bits / 4),
        bits,
    ))
}
fn integer_format(t: &Type, arch: Architecture) -> Option<(u32, bool)> {
    Some(match t {
        Type::I8 | Type::U8 => (8, matches!(t, Type::I8)),
        Type::I16 | Type::U16 => (16, matches!(t, Type::I16)),
        Type::I32 | Type::U32 => (32, matches!(t, Type::I32)),
        Type::I64 | Type::U64 => (64, matches!(t, Type::I64)),
        Type::ISize => (arch.pointer_bytes() as u32 * 8, true),
        Type::USize => (arch.pointer_bytes() as u32 * 8, false),
        _ => return None,
    })
}

/// Normalize an integer constant using its declared type and target architecture.
/// Named types fall back to the metadata value's storage type.
pub fn normalized_integer(value: &Value, t: &Type, arch: Architecture) -> Option<(i128, u32)> {
    let (bits, signed) = integer_format(t, arch).or_else(|| {
        matches!(t, Type::ValueName(_) | Type::ClassName(_))
            .then(|| integer_format(&value.ty(), arch))
            .flatten()
    })?;
    let raw = integer(value)? & ((1i128 << bits) - 1);
    let normalized = if signed && raw & (1i128 << (bits - 1)) != 0 {
        raw - (1i128 << bits)
    } else {
        raw
    };
    Some((normalized, bits))
}
pub fn attribute_value(v: &Value) -> String {
    if let Value::EnumValue(n, v) = v {
        return format!("{}.{}({})", n.namespace, n.name, attribute_value(v));
    }
    if let Some((decimal, hex, _)) = integer_text(v) {
        return format!("{decimal} ({hex}, {})", type_name(&v.ty()));
    }
    format!("{v:?}")
}
fn value(v: &Value) -> Json {
    if let Value::EnumValue(n, x) = v {
        return json!({
            "kind": "enum",
            "name": n.name,
            "namespace": n.namespace,
            "value": value(x),
        });
    }
    if let Some((decimal, hex, bits)) = integer_text(v) {
        return json!({
            "kind": "integer",
            "type": type_name(&v.ty()),
            "decimal": decimal,
            "hex": hex,
            "bits": bits,
        });
    }
    match v {
        Value::Bool(x) => json!({
            "kind": "boolean",
            "value": x,
        }),
        Value::F32(x) => json!({
            "kind": "float",
            "type": type_name(&v.ty()),
            "value": x.to_string(),
        }),
        Value::F64(x) => json!({
            "kind": "float",
            "type": type_name(&v.ty()),
            "value": x.to_string(),
        }),
        Value::Utf8(x) | Value::Utf16(x) => json!({
            "kind": "string",
            "value": x,
        }),
        Value::TypeName(n) => json!({
            "kind": "type_name",
            "name": n.name,
            "namespace": n.namespace,
        }),
        _ => unreachable!(),
    }
}
fn attributes<'a>(r: impl HasAttributes<'a>) -> Json {
    json!(
        r.attributes()
            .map(|a| {
                json!({
                    "name": a.name(),
                    "arguments": a.value().iter().map(|(n, v)| {
                        json!({
                            "name": n,
                            "value": value(v),
                        })
                    }).collect::<Vec<_>>(),
                })
            })
            .collect::<Vec<_>>()
    )
}
fn type_name(t: &Type) -> String {
    primitive_name(t).map_or_else(|| format!("{t:?}"), str::to_owned)
}
fn signature(t: &Type) -> Json {
    match t {
        Type::ClassName(n) | Type::ValueName(n) => {
            json!({
                "kind": "named",
                "name": n.name,
                "namespace": n.namespace,
                "reference_id": n.identity.map(|(source_id, token)| EntityId {
                    source_id,
                    token,
                }),
                "arguments": n.generics.iter().map(signature).collect::<Vec<_>>(),
            })
        }
        pointer @ (Type::PtrMut(t, d) | Type::PtrConst(t, d)) => {
            json!({
                "kind": "pointer",
                "depth": d,
                "const": matches!(pointer, Type::PtrConst(..)),
                "element": signature(t),
            })
        }
        Type::ArrayFixed(t, n) => json!({
            "kind": "array",
            "length": n,
            "element": signature(t),
        }),
        Type::Array(t) => json!({
            "kind": "array",
            "length": null,
            "element": signature(t),
        }),
        Type::RefMut(t) => json!({
            "kind": "byref",
            "element": signature(t),
        }),
        Type::RefConst(t) => json!({
            "kind": "const",
            "element": signature(t),
        }),
        Type::Generic(n, i) => json!({
            "kind": "generic",
            "scope": "type",
            "name": n,
            "index": i,
        }),
        Type::MethodGeneric(n, i) => json!({
            "kind": "generic",
            "scope": "method",
            "name": n,
            "index": i,
        }),
        other => json!({
            "kind": "primitive",
            "name": type_name(other),
        }),
    }
}
fn generic(p: GenericParam<'_>) -> Json {
    json!({
        "name": p.name(),
        "index": p.sequence(),
        "flags": p.flags().0,
    })
}
fn accessor(s: MethodSemantics<'_>) -> Json {
    json!({
        "semantics": s.flags(),
        "method_id": key(s.method(), 6),
        "name": s.method().name(),
    })
}
/// Member identities filtered for presentation, preserving metadata order.
pub fn member_ids<'a, R: HasAttributes<'a> + AsRow<'a>>(
    rows: impl Iterator<Item = R>,
    table: u32,
    arch: Option<Architecture>,
) -> impl Iterator<Item = EntityId> {
    rows.filter(move |r| arch.is_none_or(|a| winlex_core::metadata::supports(*r, a.bit())))
        .map(move |r| key(r, table))
}
/// Presentation name for a type's declared layout kind.
pub fn layout_kind(flags: TypeAttributes) -> &'static str {
    if flags.contains(TypeAttributes::ExplicitLayout) {
        "explicit"
    } else if flags.contains(TypeAttributes::SequentialLayout) {
        "sequential"
    } else {
        "auto"
    }
}
pub fn describe(c: &Catalog, id: EntityId, arch: Option<Architecture>) -> Result<Json> {
    describe_target(c, &id.into(), arch)
}

/// Closed instances require an explicit architecture for dependency resolution.
pub fn describe_target(
    c: &Catalog,
    target: &DeclarationTarget,
    arch: Option<Architecture>,
) -> Result<Json> {
    let selected = if target.arguments.is_empty() {
        arch.unwrap_or(Architecture::X64)
    } else {
        arch.ok_or_else(|| {
            winlex_core::Error::msg("instance description requires an architecture")
        })?
    };
    let target = if target.arguments.is_empty() {
        target.clone()
    } else {
        c.bind_target(target, selected)?
    };
    let id = target.id;
    let row = c.declaration(id, arch)?;
    let identity = match &row {
        Declaration::Type(t) if target.has_instance_identity(*t) => {
            Some(c.interface_identity(&target, selected))
        }
        _ => None,
    };
    let e = declaration::target_summary(c, &target, selected)?;
    let mut result = match row {
        Declaration::Type(t) => {
            let mut result = json!({
                "entity": e,
                "flags": t.flags().0,
                "guid": guid(t),
                "attributes": attributes(t),
                "generic_parameters": t.generic_params().map(generic).collect::<Vec<_>>(),
                "layout_constraints": t.class_layout().map(|l| json!({
                    "pack_bytes": l.packing_size(),
                    "declared_size_bytes": l.class_size(),
                })),
                "layout_kind": layout_kind(t.flags()),
                "fields": member_ids(t.fields(), 4, arch)
                    .map(|id| describe_target(c, &target.with_id(id), arch))
                    .collect::<Result<Vec<_>>>()?,
                "methods": member_ids(t.methods(), 6, arch)
                    .map(|id| describe_target(c, &target.with_id(id), arch))
                    .collect::<Result<Vec<_>>>()?,
                "properties": member_ids(t.properties(), 0x17, arch)
                    .map(|id| describe_target(c, &target.with_id(id), arch))
                    .collect::<Result<Vec<_>>>()?,
                "events": member_ids(t.events(), 0x14, arch)
                    .map(|id| describe_target(c, &target.with_id(id), arch))
                    .collect::<Result<Vec<_>>>()?,
            });
            result["interface_implementations"] = json!(
                t.interface_impls()
                    .map(|i| {
                        Ok(json!({
                            "type": signature(&target.ty(&i.interface(&t.generic_types()))?),
                            "default": i.has_attribute("DefaultAttribute"),
                            "attributes": attributes(i),
                        }))
                    })
                    .collect::<Result<Vec<_>>>()?
            );
            result["base_type"] = t
                .extends()
                .map(|b| {
                    target
                        .ty(&b.ty(&t.generic_types()))
                        .map(|ty| signature(&ty))
                })
                .transpose()?
                .unwrap_or(Json::Null);
            result["windows_runtime"] = json!(t.flags().contains(TypeAttributes::WindowsRuntime));
            result
        }
        Declaration::Property(p) => {
            let s = target.signature(p.signature())?;
            json!({
                "entity": e,
                "parent_id": key(p.parent(), 2),
                "flags": p.flags(),
                "type": signature(&s.return_type),
                "index_parameters": s.types.iter().map(signature).collect::<Vec<_>>(),
                "attributes": attributes(p),
                "accessors": p.accessors().map(accessor).collect::<Vec<_>>(),
            })
        }
        Declaration::Event(r) => {
            json!({
                "entity": e,
                "parent_id": key(r.parent(), 2),
                "flags": r.flags(),
                "type": signature(&target.ty(&r.ty())?),
                "attributes": attributes(r),
                "accessors": r.accessors().map(accessor).collect::<Vec<_>>(),
            })
        }
        Declaration::Field(f) => {
            json!({
                "entity": e,
                "parent_id": key(f.parent_row::<TypeDef>(4), 2),
                "type": signature(&target.ty(&f.ty())?),
                "declared_offset_bytes": f.explicit_offset(),
                "bitfields": winlex_core::metadata::bitfields(f)?,
                "value": f.constant().map(|v| value(&v.value())),
                "guid": guid(f),
                "attributes": attributes(f),
            })
        }
        Declaration::Method(m) => {
            let s = target.signature(m.signature(&[]))?;
            let ps = m.params_by_sequence(s.types.len())?;
            json!({
                "entity": e,
                "parent_id": key(m.parent_row::<TypeDef>(5), 2),
                "return_type": signature(&s.return_type),
                "signature_flags": s.flags.0,
                "return_attributes": ps.return_param().map(attributes),
                "attributes": attributes(m),
                "import": m.impl_map().map(|i| json!({
                    "dll": i.import_scope().name(),
                    "entry_point": i.import_name(),
                    "flags": i.flags().0,
                    "calling_convention": m.calling_convention(),
                })),
                "generic_parameters": m.generic_params().map(generic).collect::<Vec<_>>(),
                "parameters": s.types.iter().zip(ps.params()).enumerate().map(|(i, (t, p))| {
                    json!({
                        "sequence": i + 1,
                        "name": p.as_ref().map(|p| p.name()),
                        "type": signature(t),
                        "flags": p.map(|p| p.flags().0),
                        "direction": p.map(|p| format!("{:?}", p.direction())),
                        "optional": p.map(|p| p.is_optional()),
                        "attributes": p.map(attributes),
                    })
                }).collect::<Vec<_>>(),
            })
        }
        Declaration::Reference(r) => {
            json!({
                "entity": e,
                "scope_token": format!("0x{:08x}", r.scope_token()),
                "resolution": match r.foundation_type() {
                    Some(foundation) => Some(json!({"builtin": format!("{foundation:?}"), "semantics": foundation.semantics()})),
                    None => arch.map(|a| match c.resolve_reference(id, a) {
                    Ok(id) => json!({"target_id": id}),
                    Err(e) => json!({"unresolved": e.to_string()}),
                    }),
                },
            })
        }
    };
    if !target.arguments.is_empty() {
        result["type_arguments"] =
            json!(target.arguments.iter().map(signature).collect::<Vec<_>>());
        match identity {
            Some(Ok(identity)) => result["instance_identity"] = json!(identity),
            Some(Err(error)) => {
                result["instance_identity_error"] = json!(format!("{error:#}"));
            }
            None => {}
        }
    }
    Ok(result)
}
pub fn layout(l: &winlex_core::Layout) -> Json {
    fn field(f: &winlex_core::FieldLayout) -> Json {
        let mut result = json!({
            "id": f.id,
            "name": f.name,
            "offset_bits": f.offset_bits,
            "size_bits": f.size_bits,
            "bitfields": f.bitfields.iter().map(|b| json!({
                "name": b.name,
                "relative_offset_bits": b.relative_offset_bits,
                "width_bits": b.width_bits,
                "offset_bits": f.offset_bits + b.relative_offset_bits,
            })).collect::<Vec<_>>(),
        });
        if !f.fields.is_empty() {
            result["fields"] = f.fields.iter().map(field).collect();
        }
        result
    }
    json!({
        "id": l.id,
        "name": l.name,
        "architecture": l.architecture,
        "size_bits": l.size_bits,
        "alignment_bits": l.alignment_bits,
        "fields": l.fields.iter().map(field).collect::<Vec<_>>(),
        "dependencies_loaded": l.dependencies_loaded,
        "backend": "repc 0.1.1",
        "scope": "sequential and all-zero explicit overlays",
    })
}
pub fn vtable(v: &winlex_core::Vtable) -> Json {
    let mut result = json!({
        "id": v.id,
        "name": v.name,
        "architecture": v.architecture,
        "slots": v.slots.iter().enumerate().map(|(index, s)| json!({
            "method": s.method,
            "declaring_type": s.declaring_type,
            "id": s.id,
            "declaring_type_id": s.declaring_type_id,
            "index": index,
            "byte_offset": index * v.architecture.pointer_bytes(),
            "implicit": s.id.is_none(),
        })).collect::<Vec<_>>(),
    });
    if v.windows_runtime {
        result["abi"] = json!("winrt");
        result["scope"] =
            json!("method slots only; metadata signatures are not lowered ABI signatures");
    }
    if let Some(instance) = &v.instance {
        result["instance"] = json!({
            "identity": instance,
            "methods": v.slots.iter().enumerate().filter_map(|(slot, entry)| {
                let abi = entry.abi.as_ref()?;
                let mut method = json!({ "id": entry.id?, "slot": slot,
                    "offset_bytes": slot * v.architecture.pointer_bytes() });
                match abi {
                    Ok(abi) => method["abi"] = json!(abi),
                    Err(reason) => method["unsupported"] = json!(reason),
                }
                Some(method)
            }).collect::<Vec<_>>()
        });
        result["scope"] = json!(
            "WinRT slots and scalar ABI; arrays and struct/runtime-class lowering are explicitly unsupported per method"
        );
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalized_integers_follow_declared_type_and_architecture() {
        for (value, ty, arch, expected) in [
            (Value::U32(u32::MAX), Type::I32, Architecture::X64, (-1, 32)),
            (
                Value::I32(-1),
                Type::U32,
                Architecture::X64,
                (u32::MAX as i128, 32),
            ),
            (Value::U32(0x80), Type::I8, Architecture::X64, (-128, 8)),
            (
                Value::U64(u64::MAX),
                Type::ISize,
                Architecture::X86,
                (-1, 32),
            ),
            (
                Value::U64(u64::MAX),
                Type::USize,
                Architecture::X64,
                (u64::MAX as i128, 64),
            ),
        ] {
            assert_eq!(normalized_integer(&value, &ty, arch), Some(expected));
        }
        assert_eq!(
            normalized_integer(&Value::Bool(true), &Type::I32, Architecture::X64),
            None
        );
    }

    #[test]
    fn primitive_names_are_shared_by_json_text_and_attribute_annotations() {
        assert_eq!(signature(&Type::I32)["name"].as_str(), Some("int32"));
        assert_eq!(signature(&Type::ISize)["name"].as_str(), Some("nint"));
        assert_eq!(signature(&Type::Void)["name"].as_str(), Some("void"));
        assert_eq!(value(&Value::ISize(-1))["type"].as_str(), Some("nint"));
        assert_eq!(value(&Value::F32(1.0))["type"].as_str(), Some("float32"));
        assert_eq!(attribute_value(&Value::I32(1)), "1 (0x00000001, int32)");
    }

    #[test]
    fn integer_projections_preserve_signed_values_and_storage_widths() {
        for (input, decimal, hex, bits) in [
            (Value::I8(-1), "-1", "0xff", 8),
            (Value::U8(255), "255", "0xff", 8),
            (Value::I16(-32768), "-32768", "0x8000", 16),
            (Value::U16(1), "1", "0x0001", 16),
            (Value::I32(-1), "-1", "0xffffffff", 32),
            (Value::U32(u32::MAX), "4294967295", "0xffffffff", 32),
            (
                Value::I64(i64::MIN),
                "-9223372036854775808",
                "0x8000000000000000",
                64,
            ),
            (
                Value::U64(u64::MAX),
                "18446744073709551615",
                "0xffffffffffffffff",
                64,
            ),
            (Value::ISize(-1), "-1", "0xffffffffffffffff", 64),
            (Value::USize(1), "1", "0x0000000000000001", 64),
        ] {
            let json = value(&input);
            assert_eq!(json["decimal"], decimal);
            assert_eq!(json["hex"], hex);
            assert_eq!(json["bits"], bits);
            assert_eq!(constant(&input), decimal);
            assert_eq!(
                attribute_value(&input),
                format!("{decimal} ({hex}, {})", type_name(&input.ty()))
            );
        }
    }
}
