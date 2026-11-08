fn source(name: &str, bytes: Vec<u8>) -> winlex_core::SourceData {
    let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tmp");
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join(format!("{name}-{}.winmd", std::process::id()));
    std::fs::write(&path, bytes).unwrap();
    let source = winlex_core::SourceData::read(&path).unwrap();
    std::fs::remove_file(path).unwrap();
    source
}
