//! CLI harness for development and integration checks.
fn main() -> std::process::ExitCode {
    let code = winlex_cli::run(
        std::env::args_os().skip(1),
        &mut std::io::stdout().lock(),
        &mut std::io::stderr().lock(),
    );
    std::process::ExitCode::from(code)
}
