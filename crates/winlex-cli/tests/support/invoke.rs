fn invoke(args: impl IntoIterator<Item = OsString>) -> (u8, String, String) {
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let code = winlex_cli::run(args, &mut out, &mut err);
    (
        code,
        String::from_utf8(out).unwrap(),
        String::from_utf8(err).unwrap(),
    )
}
