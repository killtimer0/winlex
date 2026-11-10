use std::{env, fs, path::PathBuf};

fn main() {
    // Use the SDK import library for the common-control subclass helpers.
    println!("cargo:rustc-link-lib=comctl32");
    println!("cargo:rerun-if-changed=res/resource.h");
    println!("cargo:rerun-if-changed=Cargo.toml");
    println!("cargo:rerun-if-changed=../../Cargo.toml");
    println!("cargo:rerun-if-changed=res/gui.manifest");
    let version = env::var("CARGO_PKG_VERSION").expect("package version");
    let template = fs::read_to_string("res/gui.manifest").expect("GUI manifest template");
    assert!(
        template.contains("{{VERSION}}"),
        "GUI manifest version placeholder"
    );
    let manifest = PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("gui.manifest");
    fs::write(
        &manifest,
        template.replace("{{VERSION}}", &format!("{version}.0")),
    )
    .expect("Write GUI manifest");
    println!(
        "cargo:rustc-env=WINLEX_GUI_MANIFEST={}",
        manifest
            .to_str()
            .expect("GUI manifest path must be valid Unicode")
    );
    let header = fs::read_to_string("res/resource.h").expect("resource IDs");
    let mut rust = String::new();
    let mut strings = Vec::new();
    for line in header.lines().filter(|s| s.starts_with("#define ")) {
        let fields: Vec<_> = line.split_whitespace().collect();
        assert_eq!(
            fields.len(),
            3,
            "Resource IDs must be simple decimal definitions"
        );
        let value: u16 = fields[2].parse().expect("numeric resource ID");
        rust.push_str(&format!("pub const {}: u16 = {value};\n", fields[1]));
        if fields[1].starts_with("IDS_") {
            strings.push(fields[1]);
        }
    }
    rust.push_str(&format!(
        "pub const STRING_IDS: &[u16] = &[{}];\n",
        strings.join(",")
    ));
    fs::write(
        PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("resource_ids.rs"),
        rust,
    )
    .unwrap();
}
