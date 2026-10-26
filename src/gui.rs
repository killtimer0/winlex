#![windows_subsystem = "windows"]

fn main() {
    if let Err(error) = winlex_gui::run(std::env::args().skip(1)) {
        winlex_gui::show_error(&error);
        std::process::exit(1);
    }
}
