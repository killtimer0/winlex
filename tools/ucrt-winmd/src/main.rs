use std::error::Error;

fn main() -> Result<(), Box<dyn Error>> {
    std::env::set_current_dir(env!("CARGO_MANIFEST_DIR"))?;
    let arch = std::env::args().nth(1).unwrap_or_else(|| "x64".into());
    if arch == "merge" {
        let mut merger = windows_metadata::merge::Merger::new();
        for (name, bits) in [("x86", 1), ("x64", 2), ("arm64", 4)] {
            merger.arch_input(format!("out/{name}/UCRT.winmd"), bits);
        }
        merger.output("out/UCRT.winmd").merge()?;
        roundtrip(std::path::Path::new("out"))?;
        return Ok(());
    }
    let target = match arch.as_str() {
        "x86" => "i686-pc-windows-msvc",
        "x64" => "x86_64-pc-windows-msvc",
        "arm64" => "aarch64-pc-windows-msvc",
        _ => return Err("expected x86, x64 or arm64".into()),
    };
    let out = std::path::PathBuf::from("out").join(&arch);
    std::fs::create_dir_all(&out)?;
    let config: serde_json::Value = serde_json::from_slice(
        &std::fs::read("out/toolchain.json")
            .map_err(|error| format!("{error}; configure the toolchain with build.ps1 first"))?,
    )?;
    let mut args = clang_args(&config)?;
    args.push(format!("--target={target}"));
    let clang_version = windows_clang::clang_version()?;
    println!("{clang_version}");
    if out.join("rdl").exists() {
        std::fs::remove_dir_all(out.join("rdl"))?;
    }
    windows_clang::clang()
        .input("main.h")
        .args(args)
        .scope_headers(["stdio.h", "stdlib.h", "string.h", "time.h", "stat.h"])
        .scope("ucrt")
        .scope_headers(["vadefs.h", "vcruntime.h", "vcruntime_startup.h", "stddef.h"])
        .namespace("Ucrt.CRuntime")
        .library("")
        .output(out.join("rdl"))
        .write_by_header()?;
    for entry in std::fs::read_dir(out.join("rdl"))? {
        let path = entry?.path();
        if path.extension().is_some_and(|extension| extension == "rdl") {
            let source = std::fs::read_to_string(&path)?;
            std::fs::write(&path, source.replace("#[library(\"\")]", "#[declaration]"))?;
        }
    }
    windows_rdl::reader()
        .input(out.join("rdl"))
        .input("strings.rdl")
        .input("declarations.rdl")
        .reference_default()
        .output(out.join("UCRT.untagged.winmd"))
        .write()?;
    let bits = match arch.as_str() {
        "x86" => 1,
        "x64" => 2,
        _ => 4,
    };
    windows_metadata::merge::Merger::new()
        .arch_input(out.join("UCRT.untagged.winmd"), bits)
        .output(out.join("UCRT.winmd"))
        .merge()?;
    roundtrip(&out)?;
    std::fs::write(
        out.join("generation.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "architecture": arch, "target": target, "libclang_version": clang_version,
        }))?,
    )?;
    Ok(())
}

fn clang_args(config: &serde_json::Value) -> Result<Vec<String>, Box<dyn Error>> {
    let args: Vec<String> = serde_json::from_value(config["clang_args"].clone())?;
    if args.is_empty() {
        return Err("toolchain clang_args must not be empty".into());
    }
    Ok(args)
}

fn roundtrip(out: &std::path::Path) -> Result<(), Box<dyn Error>> {
    windows_rdl::writer()
        .input(out.join("UCRT.winmd"))
        .output(out.join("roundtrip.rdl"))
        .write()?;
    windows_rdl::reader()
        .input(out.join("roundtrip.rdl"))
        .reference_default()
        .output(out.join("UCRT.roundtrip.winmd"))
        .write()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toolchain_args_preserve_unicode_spaces_and_unc_paths() {
        let args = vec![
            "-x",
            "c",
            "-IC:/SDK 路径/ucrt",
            "-I\\\\server\\share\\include",
        ];
        assert_eq!(
            clang_args(&serde_json::json!({"clang_args": args})).unwrap(),
            args
        );
    }

    #[test]
    fn toolchain_rejects_missing_empty_or_nonstring_args() {
        for value in [
            serde_json::json!({}),
            serde_json::json!({"clang_args": []}),
            serde_json::json!({"clang_args": [1]}),
        ] {
            assert!(clang_args(&value).is_err());
        }
    }
}
