pub(crate) fn metadata_path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../inputs/Windows.Win32.winmd")
}
