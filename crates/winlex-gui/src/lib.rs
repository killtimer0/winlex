#![cfg(windows)]

mod app;
mod config;
mod dialog;
mod dpi;
mod hidden;
mod icons;
mod input;
mod layout;
mod list;
mod message;
mod native;
mod platform;
mod resources;
mod results;
mod rich_text;
mod tabs;
mod tasks;
#[cfg(test)]
mod test_support;
mod theme;
mod worker;

pub fn run(args: impl Iterator<Item = std::ffi::OsString>) -> anyhow::Result<()> {
    use anyhow::{Context, bail};
    let args: Vec<_> = args.collect();
    if hidden::requested(&args) {
        hidden::run(&args);
        return Ok(());
    }
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
    message::startup_error(message);
}
