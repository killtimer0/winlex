#![cfg(windows)]

mod app;
mod config;
mod dpi;
mod input;
mod layout;
mod native;
mod platform;
mod resources;
mod rich_text;
mod tasks;
mod worker;

pub fn run(args: impl Iterator<Item = std::ffi::OsString>) -> anyhow::Result<()> {
    use anyhow::{Context, anyhow, bail};
    let mode = match std::env::var("WINLEX_DPI") {
        Ok(value) => value.parse().map_err(anyhow::Error::msg)?,
        Err(std::env::VarError::NotPresent) => dpi::Mode::Auto,
        Err(_) => bail!("WINLEX_DPI must be valid Unicode"),
    };
    let mut args = args;
    let mut paths = Vec::new();
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--winmd") => paths.push(
                std::path::absolute(
                    args.next()
                        .ok_or_else(|| anyhow!("--winmd requires a path"))?,
                )
                .context("Resolve input path")?,
            ),
            _ => {
                bail!("Unknown argument: {arg:?}\nUsage: winlex-gui [--winmd PATH ...]");
            }
        }
    }
    let config = config::Config::load(1).map_err(anyhow::Error::msg)?;
    app::run(dpi::Dpi::initialize(mode), config, paths).map_err(anyhow::Error::msg)
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
