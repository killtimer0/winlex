use std::{env, fs, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=WINLEX_YY_THUNKS_OBJ");
    println!("cargo:rerun-if-env-changed=WINLEX_VC_LTL_ROOT");
    let target = env::var("TARGET").expect("TARGET");
    let (machine, subsystem) = match target.as_str() {
        "i686-pc-windows-msvc" => (0x014cu16, "5.01"),
        "x86_64-pc-windows-msvc" => (0x8664u16, "5.02"),
        _ => panic!(
            "Winlex requires i686-pc-windows-msvc or x86_64-pc-windows-msvc and MSVC link.exe. Use scripts/build.ps1."
        ),
    };
    let features = env::var("CARGO_CFG_TARGET_FEATURE").unwrap_or_default();
    assert!(
        features.split(',').any(|f| f == "crt-static"),
        "VC-LTL integration requires +crt-static; use scripts/build.ps1"
    );
    let ltl_root = PathBuf::from(
        env::var_os("WINLEX_VC_LTL_ROOT")
            .expect("Set WINLEX_VC_LTL_ROOT to the official VC-LTL5 SDK directory"),
    );
    let platform = if machine == 0x014c {
        "5.1.2600.0/lib/Win32"
    } else {
        "5.2.3790.0/lib/x64"
    };
    let libdir = fs::canonicalize(ltl_root.join("TargetPlatform").join(platform))
        .expect("VC-LTL library directory for the minimum Windows version is missing");
    for name in ["libucrt.lib", "libvcruntime.lib"] {
        let path = libdir.join(name);
        assert!(path.is_file(), "Missing VC-LTL library: {}", path.display());
        println!("cargo:rerun-if-changed={}", path.display());
    }
    // Like upstream's Rust integration, place the replacement CRT libraries
    // on the native search path; +crt-static still selects the /MT library names.
    println!("cargo:rustc-link-search=native={}", libdir.display());
    let object = PathBuf::from(
        env::var_os("WINLEX_YY_THUNKS_OBJ")
            .expect("Set WINLEX_YY_THUNKS_OBJ to the matching official YY_Thunks_for_WinXP.obj"),
    );
    let object = fs::canonicalize(object).expect("YY-Thunks object does not exist");
    let bytes = fs::read(&object).expect("Cannot read YY-Thunks object");
    assert!(bytes.len() >= 20, "Truncated COFF object");
    assert_eq!(
        u16::from_le_bytes([bytes[0], bytes[1]]),
        machine,
        "YY-Thunks COFF architecture does not match the Rust target"
    );
    println!("cargo:rerun-if-changed={}", object.display());
    // Upstream's object route specifically requires MSVC link.exe, not lld-link.
    println!("cargo:rustc-link-arg-bin=winlex={}", object.display());
    println!("cargo:rustc-link-arg-bin=winlex=/SUBSYSTEM:CONSOLE,{subsystem}");
}
