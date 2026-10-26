//! Exhaustive declaration decoding for supplied verification inputs.
use std::path::PathBuf;
use windows_metadata::reader::{AsRow, TypeDef};
use winlex_core::{Architecture, Catalog, metadata::Db};

fn main() -> winlex_core::Result<()> {
    let paths: Vec<PathBuf> = std::env::args_os().skip(1).map(Into::into).collect();
    let db = Db::open_many(&paths)?;
    let catalog = Catalog::open_many(&paths)?;
    let mut count = 0;
    for (file, source) in db.sources.iter().enumerate() {
        for ty in db.index.rows::<TypeDef>(file) {
            if ty.name() == "<Module>" {
                continue;
            }
            let id = format!("{}::0x{:08x}", source.id, 0x02000000 | (ty.pos() + 1));
            catalog.describe(&id, Some(Architecture::X64))?;
            count += 1;
        }
    }
    println!("Decoded {count} complete type declarations");
    Ok(())
}
