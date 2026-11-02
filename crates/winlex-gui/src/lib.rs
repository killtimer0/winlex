#![cfg(windows)]

mod app;
mod config;
mod dialog;
mod dpi;
mod icons;
mod input;
mod layout;
mod list;
mod native;
mod platform;
mod resources;
mod results;
mod rich_text;
mod tabs;
mod tasks;
mod worker;

pub fn run(args: impl Iterator<Item = std::ffi::OsString>) -> anyhow::Result<()> {
    use anyhow::{Context, bail};
    let mode = match std::env::var("WINLEX_DPI") {
        Ok(value) => value.parse()?,
        Err(std::env::VarError::NotPresent) => dpi::Mode::Auto,
        Err(_) => bail!("WINLEX_DPI must be valid Unicode"),
    };
    let mut paths = Vec::new();
    for arg in args {
        if arg.to_str().is_some_and(|value| value.starts_with("--")) {
            bail!("Unknown argument: {arg:?}\nUsage: winlex-gui [FILE ...]");
        }
        paths.push(std::path::absolute(arg).context("Resolve input path")?);
    }
    let config = config::Config::load(1)?;
    app::run(dpi::Dpi::initialize(mode), config, paths)
}

pub fn show_error(message: &str) {
    unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::MessageBoxW(
            std::ptr::null_mut(),
            native::wide(message).as_ptr(),
            windows_sys::w!("Winlex"),
            windows_sys::Win32::UI::WindowsAndMessaging::MB_OK
                | windows_sys::Win32::UI::WindowsAndMessaging::MB_ICONERROR,
        );
    }
}
