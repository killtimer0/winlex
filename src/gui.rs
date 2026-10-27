#![windows_subsystem = "windows"]

fn main() {
    if let Err(error) = winlex_gui::run(std::env::args_os().skip(1)) {
        winlex_gui::show_error(&error);
        std::process::exit(1);
    }
}
