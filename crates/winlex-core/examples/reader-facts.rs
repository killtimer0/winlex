//! Raw reader output for comparison with System.Reflection.Metadata.
fn main() -> anyhow::Result<()> {
    let path = std::env::args_os()
        .nth(1)
        .ok_or_else(|| anyhow::anyhow!("usage: reader-facts FILE"))?;
    let db = winlex_core::metadata::Db::open(std::path::Path::new(&path))?;
    serde_json::to_writer(std::io::stdout().lock(), &db.reader_facts())?;
    Ok(())
}
