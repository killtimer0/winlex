pub(crate) fn metadata_path() -> std::path::PathBuf {
    std::env::var_os("WINLEX_TEST_WINMD")
        .map(std::path::PathBuf::from)
        .expect("Set WINLEX_TEST_WINMD to the Windows.Win32.winmd test input")
}
