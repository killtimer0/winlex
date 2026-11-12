//! Exhaustive declaration decoding for supplied verification inputs.
use std::path::PathBuf;
use winlex_core::{Architecture, Catalog};
fn main() -> winlex_core::Result<()> {
    let paths: Vec<PathBuf> = std::env::args_os().skip(1).map(Into::into).collect();
    let catalog = Catalog::open_many(&paths)?;
    let mut count = 0;
    for id in catalog.type_ids() {
        if catalog.declaration(id, None)?.name() == "<Module>" {
            continue;
        }
        winlex_format::describe_target(&catalog, &id.into(), Some(Architecture::X64))?;
        count += 1;
    }
    println!("Decoded {count} complete type declarations");
    Ok(())
}
