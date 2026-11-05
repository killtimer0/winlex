use std::{env, fs, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=WINLEX_YY_THUNKS_OBJ");
    println!("cargo:rerun-if-env-changed=WINLEX_VC_LTL_ROOT");
    let target = env::var("TARGET").expect("TARGET");
    println!("cargo:rerun-if-changed=inputs/targets.json");
    let targets: serde_json::Value =
        serde_json::from_str(include_str!("inputs/targets.json")).expect("target policies");
    let policy = targets
        .as_object()
        .unwrap()
        .values()
        .find(|policy| policy["target"].as_str() == Some(&target))
        .expect("Unsupported Rust target; use scripts/build.ps1");
    let subsystem = policy["subsystem"].as_str().expect("subsystem version");
    let platform = policy["vc_ltl_platform"].as_str().expect("VC-LTL platform");
    let features = env::var("CARGO_CFG_TARGET_FEATURE").unwrap_or_default();
    assert!(
        features.split(',').any(|f| f == "crt-static"),
        "Winlex requires +crt-static; use scripts/build.ps1"
    );
    let ltl_root = PathBuf::from(
        env::var_os("WINLEX_VC_LTL_ROOT")
            .expect("Set WINLEX_VC_LTL_ROOT to the official VC-LTL5 SDK directory"),
    );
    let libdir = fs::canonicalize(ltl_root.join("TargetPlatform").join(platform))
        .expect("VC-LTL library directory for the minimum Windows version is missing");
    for name in ["libucrt.lib", "libvcruntime.lib"] {
        let path = libdir.join(name);
        assert!(path.is_file(), "Missing VC-LTL library: {}", path.display());
        println!("cargo:rerun-if-changed={}", path.display());
    }
    println!("cargo:rerun-if-changed={}", libdir.display());
    // Like upstream's Rust integration, place the replacement CRT libraries
    // on the native search path; +crt-static still selects the /MT library names.
    println!("cargo:rustc-link-search=native={}", libdir.display());
    if policy["thunks_arch"].is_string() {
        let object =
            PathBuf::from(env::var_os("WINLEX_YY_THUNKS_OBJ").expect(
                "Set WINLEX_YY_THUNKS_OBJ to the matching official YY_Thunks_for_WinXP.obj",
            ));
        let object = fs::canonicalize(object).expect("YY-Thunks object does not exist");
        let bytes = fs::read(&object).expect("Cannot read YY-Thunks object");
        assert!(bytes.len() >= 20, "Truncated COFF object");
        assert_eq!(
            u16::from_le_bytes([bytes[0], bytes[1]]),
            policy["machine"].as_u64().expect("PE machine") as u16,
            "YY-Thunks COFF architecture does not match the Rust target"
        );
        println!("cargo:rerun-if-changed={}", object.display());
        // Upstream's object route specifically requires MSVC link.exe, not lld-link.
        println!("cargo:rustc-link-arg-bin=winlex={}", object.display());
        println!("cargo:rustc-link-arg-bin=winlex-gui={}", object.display());
    }
    println!("cargo:rustc-link-arg-bin=winlex=/SUBSYSTEM:CONSOLE,{subsystem}");
    println!("cargo:rustc-link-arg-bin=winlex-gui=/SUBSYSTEM:WINDOWS,{subsystem}");
    compile_gui_resources();
}

fn compile_gui_resources() {
    println!("cargo:rerun-if-env-changed=WINLEX_RC_EXE");
    println!("cargo:rerun-if-changed=crates/winlex-gui/res");
    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let version = env::var("CARGO_PKG_VERSION").unwrap();
    let numbers: Vec<_> = version
        .split('.')
        .map(|n| n.parse::<u16>().expect("numeric package version"))
        .collect();
    assert_eq!(numbers.len(), 3);
    fs::write(
        out.join("version.h"),
        format!(
            "#define VERSION_NUMBER {},{},{},0\n#define VERSION_TEXT \"{version}.0\"\n",
            numbers[0], numbers[1], numbers[2]
        ),
    )
    .unwrap();
    let resource = out.join("winlex-gui.res");
    let rc = env::var_os("WINLEX_RC_EXE")
        .expect("Set WINLEX_RC_EXE to Microsoft rc.exe; use scripts/build.ps1");
    let status = std::process::Command::new(rc)
        .current_dir(root.join("crates/winlex-gui/res"))
        .arg("/nologo")
        .arg("/c65001")
        .arg("/i")
        .arg(&out)
        .arg("/fo")
        .arg(&resource)
        .arg("gui.rc")
        .status()
        .expect("Run Microsoft resource compiler");
    assert!(status.success(), "GUI resource compilation failed");
    println!("cargo:rustc-link-arg-bin=winlex-gui={}", resource.display());
    // The RC resource is the single owner of manifest ID 1.
    println!("cargo:rustc-link-arg-bin=winlex-gui=/MANIFEST:NO");
}
