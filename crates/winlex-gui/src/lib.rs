#![cfg(windows)]

mod app;
mod config;
mod dpi;
mod input;
mod layout;
mod native;
mod resources;
mod worker;

pub fn run(args: impl Iterator<Item = std::ffi::OsString>) -> Result<(), String> {
    let mode = match std::env::var("WINLEX_DPI") {
        Ok(value) => value.parse()?,
        Err(std::env::VarError::NotPresent) => dpi::Mode::Auto,
        Err(_) => return Err("WINLEX_DPI must be valid Unicode".into()),
    };
    let mut args = args;
    let mut paths = Vec::new();
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--winmd") => paths.push(
                std::path::absolute(args.next().ok_or("--winmd requires a path")?)
                    .map_err(|e| format!("Resolve input path: {e}"))?,
            ),
            _ => {
                return Err(format!(
                    "Unknown argument: {arg:?}\nUsage: winlex-gui [--winmd PATH ...]"
                ));
            }
        }
    }
    let config = config::Config::load()?;
    resources::initialize(config.language.name())?;
    let paths = config.startup_paths(paths);
    app::run(dpi::Dpi::initialize(mode)?, config, paths)
}

pub fn show_error(message: &str) {
    unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::MessageBoxW(
            std::ptr::null_mut(),
            native::wide(message).as_ptr(),
            native::wide("Winlex").as_ptr(),
            windows_sys::Win32::UI::WindowsAndMessaging::MB_OK
                | windows_sys::Win32::UI::WindowsAndMessaging::MB_ICONERROR,
        );
    }
}
