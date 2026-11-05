//! Vendor reader facts consumed only by tests/differential.
use serde_json::{Value as Json, json};
use winlex_core::{
    metadata::{Db, token},
    windows_metadata::{Signature, Type, Value, reader::*},
};

fn ty(t: &Type) -> Json {
    match t {
        Type::ClassName(n) | Type::ValueName(n) => json!({
            "kind": if matches!(t, Type::ClassName(_)) { "class" } else { "value" },
            "token": n.identity.map(|(_, token)| token),
            "arguments": n.generics.iter().map(ty).collect::<Vec<_>>(),
        }),
        Type::Generic(_, index) => json!({"kind": "var", "index": index}),
        Type::MethodGeneric(_, index) => json!({"kind": "mvar", "index": index}),
        Type::Array(t) => json!({"kind": "array", "element": ty(t)}),
        Type::ArrayFixed(t, size) => json!({"kind": "shaped_array", "element": ty(t),
            "rank": 1, "sizes": [size], "lower_bounds": [0]}),
        Type::PtrMut(element, count) | Type::PtrConst(element, count) => {
            let mut result = ty(element);
            for _ in 0..*count {
                result = json!({"kind": "pointer", "element": result});
            }
            if matches!(t, Type::PtrConst(_, _)) {
                result = json!({"kind": "const", "element": result});
            }
            result
        }
        Type::RefMut(t) => json!({"kind": "byref", "element": ty(t)}),
        Type::RefConst(t) => json!({"kind": "const_byref", "element": ty(t)}),
        _ => json!({"kind": format!("{t:?}").to_lowercase()}),
    }
}

fn signature(s: Signature, mut blob: Blob<'_>) -> Json {
    let flags = blob.read_u8();
    let generic_arity = if flags & 0x10 != 0 {
        blob.read_compressed()
    } else {
        0
    };
    while !blob.is_empty() {
        blob.read_u8();
    }
    json!({"flags": s.flags.0, "generic_arity": generic_arity, "required_parameters": s.types.len(), "return": ty(&s.return_type),
        "parameters": s.types.iter().map(ty).collect::<Vec<_>>()})
}

fn value(v: Value) -> Json {
    match v {
        Value::Bool(v) => json!(v),
        Value::U8(v) => json!(v.to_string()),
        Value::I8(v) => json!(v.to_string()),
        Value::U16(v) => json!(v.to_string()),
        Value::I16(v) => json!(v.to_string()),
        Value::U32(v) => json!(v.to_string()),
        Value::I32(v) => json!(v.to_string()),
        Value::U64(v) | Value::USize(v) => json!(v.to_string()),
        Value::I64(v) | Value::ISize(v) => json!(v.to_string()),
        Value::F32(v) => json!(format!("{:08x}", v.to_bits())),
        Value::F64(v) => json!(format!("{:016x}", v.to_bits())),
        Value::Utf8(v) | Value::Utf16(v) => json!(v),
        Value::EnumValue(_, v) => value(*v),
        rest => panic!("unsupported constant: {rest:?}"),
    }
}

fn rows<'a, R: AsRow<'a> + 'a>(db: &'a Db, table: u32, f: impl Fn(R) -> Json) -> Json {
    db.index
        .rows::<R>(0)
        .map(|r| (format!("0x{:08x}", token(r, table)), f(r)))
        .collect::<serde_json::Map<_, _>>()
        .into()
}

fn blob_hex(mut blob: Blob<'_>) -> String {
    let mut result = String::new();
    while !blob.is_empty() {
        use std::fmt::Write;
        write!(result, "{:02x}", blob.read_u8()).unwrap();
    }
    result
}

fn accessors<'a>(items: impl Iterator<Item = MethodSemantics<'a>>) -> Vec<Json> {
    let mut items: Vec<_> = items
        .map(|r| json!({"method": token(r.method(), 6), "flags": r.flags()}))
        .collect();
    items.sort_by_key(|r| (r["method"].as_u64(), r["flags"].as_u64()));
    items
}

fn facts(db: &Db) -> Json {
    json!({
        "assembly": db.sources[0].assembly,
        "references": db.sources[0].references,
        "types": rows::<TypeDef>(db, 2, |r| json!({"name": r.name(), "namespace": r.namespace(),
            "flags": r.flags().0, "extends": r.extends().map(|t| t.identity().1),
            "fields": r.fields().map(|f| token(f, 4)).collect::<Vec<_>>(),
            "methods": r.methods().map(|m| token(m, 6)).collect::<Vec<_>>(),
            "layout": r.class_layout().map(|l| json!({"packing": l.packing_size(), "size": l.class_size()}))})),
        "scopes": rows::<TypeRef>(db, 1, |r| json!({"name": r.name(), "namespace": r.namespace(), "scope": r.scope_token()})),
        "fields": rows::<Field>(db, 4, |r| json!({"name": r.name(), "flags": r.flags().0,
            "type": ty(&r.ty()), "offset": r.explicit_offset()})),
        "methods": rows::<MethodDef>(db, 6, |r| json!({"name": r.name(), "flags": r.flags().0,
            "impl_flags": r.impl_flags().0, "signature": signature(r.signature(&[]), r.blob(4)),
            "parameters": r.params().map(|p| token(p, 8)).collect::<Vec<_>>(),
            "import": r.impl_map().map(|i| json!({"flags": i.flags().0, "name": i.import_name(), "module": i.import_scope().name()}))})),
        "parameters": rows::<MethodParam>(db, 8, |r| json!({"name": r.name(), "sequence": r.sequence(), "flags": r.flags().0})),
        "generics": rows::<GenericParam>(db, 0x2a, |r| json!({"name": r.name(), "index": r.sequence(), "flags": r.flags().0,
            "owner": match r.decode::<TypeOrMethodDef>(2) { TypeOrMethodDef::TypeDef(t) => token(t, 2), TypeOrMethodDef::MethodDef(m) => token(m, 6) }})),
        "constants": rows::<Constant>(db, 0xb, |r| json!({"parent_code": r.usize(1), "type": ty(&r.ty()), "value": value(r.value())})),
        "properties": rows::<Property>(db, 0x17, |r| json!({"name": r.name(), "flags": r.flags(), "parent": token(r.parent(), 2),
            "signature": signature(r.signature(), r.blob(2)), "accessors": accessors(r.accessors())})),
        "events": rows::<Event>(db, 0x14, |r| json!({"name": r.name(), "flags": r.flags(), "parent": token(r.parent(), 2),
            "type": ty(&r.ty()), "accessors": accessors(r.accessors())})),
        "attributes_raw": rows::<Attribute>(db, 0xc, |r| json!({"parent_code": r.usize(0), "constructor_code": r.usize(1),
            "blob": blob_hex(r.blob(2))})),
        "attributes_decoded": db.index.rows::<Attribute>(0).filter(|r| selected_attribute(r.namespace(), r.name())).map(|r| {
            let mut arguments = r.value().into_iter().map(|(name, v)| json!({"name": name, "value": value(v)})).collect::<Vec<_>>();
            arguments.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
            (format!("0x{:08x}", token(r, 0xc)), json!({"namespace": r.namespace(), "name": r.name(), "arguments": arguments}))
        }).collect::<serde_json::Map<_, _>>(),
        "layouts": db.index.rows::<FieldLayout>(0).map(|r| (format!("0x{:08x}", token(r.field(), 4)), json!({"offset": r.offset()}))).collect::<serde_json::Map<_, _>>(),
        "interfaces": rows::<InterfaceImpl>(db, 9, |r| json!({"owner": 0x02000000 | r.usize(0), "interface_code": r.usize(1)})),
        "nested": db.index.rows::<NestedClass>(0).map(|r| (format!("0x{:08x}", 0x02000000 | r.usize(0)), json!({"parent": 0x02000000 | r.usize(1)}))).collect::<serde_json::Map<_, _>>(),
    })
}

fn selected_attribute(namespace: &str, name: &str) -> bool {
    match namespace {
        "Windows.Win32.Foundation.Metadata" => matches!(
            name,
            "GuidAttribute"
                | "SupportedArchitectureAttribute"
                | "NativeArrayInfoAttribute"
                | "MemorySizeAttribute"
                | "DocumentationAttribute"
                | "SupportedOSPlatformAttribute"
                | "FlexibleArrayAttribute"
        ),
        "Windows.Foundation.Metadata" => matches!(
            name,
            "GuidAttribute" | "OverloadAttribute" | "DefaultAttribute"
        ),
        _ => false,
    }
}

fn main() -> winlex_core::Result<()> {
    let path = std::env::args_os()
        .nth(1)
        .ok_or_else(|| winlex_core::Error::msg("usage: winmd-facts FILE"))?;
    let db = Db::from_inputs([winlex_core::SourceData::read(path)?])?;
    serde_json::to_writer(std::io::stdout().lock(), &facts(&db))?;
    Ok(())
}
