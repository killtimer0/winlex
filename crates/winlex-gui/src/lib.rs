#![cfg(windows)]

mod app;
mod dpi;
mod layout;
mod native;
mod resources;

pub fn run(args: impl Iterator<Item = String>) -> Result<(), String> {
    let mut mode = dpi::Mode::Auto;
    let mut language = String::from("auto");
    let mut args = args;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--dpi" => mode = args.next().ok_or("--dpi requires a mode")?.parse()?,
            "--lang" => language = args.next().ok_or("--lang requires a language")?,
            _ => {
                return Err(format!(
                    "Unknown argument: {arg}\nUsage: winlex-gui [--dpi auto|pmv2|pmv1|system|unaware] [--lang auto|en-US|zh-CN]"
                ));
            }
        }
    }
    resources::initialize(&language)?;
    app::run(dpi::Dpi::initialize(mode)?)
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
