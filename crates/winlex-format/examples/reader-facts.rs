//! Raw reader output for comparison with System.Reflection.Metadata.
use serde_json::{Value as Json, json};
use winlex_core::{
    metadata::{Db, token},
    windows_metadata::{Type, reader::*},
};
fn facts(db: &Db) -> Json {
    assert_eq!(db.sources.len(), 1);
    fn refs(t: &Type, out: &mut Vec<u32>) {
        match t {
            Type::ValueName(n) | Type::ClassName(n) => {
                if let Some((_, token)) = n.identity {
                    out.push(token);
                }
                for g in &n.generics {
                    refs(g, out);
                }
            }
            Type::Array(t)
            | Type::ArrayFixed(t, _)
            | Type::PtrMut(t, _)
            | Type::PtrConst(t, _)
            | Type::RefMut(t)
            | Type::RefConst(t) => refs(t, out),
            _ => {}
        }
    }
    let layouts: Vec<_> = db
        .index
        .rows::<FieldLayout>(0)
        .map(|r| json!({"field":token(r.field(),4),"offset":r.offset()}))
        .collect();
    let scopes:Vec<_>=db.index.rows::<TypeRef>(0).map(|r|json!({"token":token(r,1),"scope":r.scope_token(),"name":r.name(),"namespace":r.namespace()})).collect();
    let mut signatures = std::collections::BTreeMap::new();
    for f in db.index.rows::<Field>(0) {
        let mut r = vec![];
        refs(&f.ty(), &mut r);
        signatures.insert(token(f, 4).to_string(), r);
    }
    for m in db.index.rows::<MethodDef>(0) {
        let s = m.signature(&[]);
        let mut r = vec![];
        refs(&s.return_type, &mut r);
        for t in &s.types {
            refs(t, &mut r);
        }
        signatures.insert(token(m, 6).to_string(), r);
    }
    let properties:Vec<_>=db.index.rows::<Property>(0).map(|p|json!({"token":token(p,0x17),"name":p.name(),"parent":token(p.parent(),2),"accessors":p.accessors().map(|s|json!({"flags":s.flags(),"method":token(s.method(),6)})).collect::<Vec<_>>()})).collect();
    let events:Vec<_>=db.index.rows::<Event>(0).map(|e|json!({"token":token(e,0x14),"name":e.name(),"parent":token(e.parent(),2),"accessors":e.accessors().map(|s|json!({"flags":s.flags(),"method":token(s.method(),6)})).collect::<Vec<_>>()})).collect();
    json!({"assembly":db.sources[0].assembly,"references":db.sources[0].references,"layouts":layouts,"scopes":scopes,"signature_type_tokens":signatures,"properties":properties,"events":events})
}
fn main() -> winlex_core::Result<()> {
    let path = std::env::args_os()
        .nth(1)
        .ok_or_else(|| winlex_core::Error::msg("usage: reader-facts FILE"))?;
    let db = Db::from_inputs([winlex_core::SourceData::read(path)?])?;
    serde_json::to_writer(std::io::stdout().lock(), &facts(&db))?;
    Ok(())
}
