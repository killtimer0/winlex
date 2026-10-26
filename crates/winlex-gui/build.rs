use std::{env, fs, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=res/resource.h");
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
