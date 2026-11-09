"""Create checked license bundles and assemble a three-architecture delivery without rebuilding."""
import argparse
import hashlib
import importlib
import json
from pathlib import Path
import shutil
import tempfile

record = importlib.import_module("build-record")
ROOT = Path(__file__).resolve().parents[1]
ARCHITECTURES = ("x86", "x64", "arm64")
NOTICES = {"LICENSE", "THIRD-PARTY-NOTICES.md"}
PROGRAM_FILES = NOTICES | {"winlex.exe", "winlex-gui.exe", "LICENSES.txt"}
REQUIRED_LICENSES = NOTICES | {
    "licenses/YY-Thunks-LICENSE", "licenses/win32-darkmodelib-MPL-2.0.md",
    "licenses/win32-darkmodelib-UAHMenuBar.md", "licenses/win32-darkmode-MIT.md",
    "licenses/VC-LTL-LICENSE", "licenses/VC-LTL-sdk-license.rtf",
    "licenses/VC-LTL-sdk-third-party-notices.rtf", "vendor/PATCHES.md",
    "vendor/windows-metadata/license-mit", "vendor/windows-metadata/license-apache-2.0",
    "crates/winlex-gui/res/codicons/LICENSE", "licenses/rust/COPYRIGHT-library.html",
}


def digest(data):
    return hashlib.sha256(data).hexdigest()


def verify_licenses(files):
    record.check(REQUIRED_LICENSES <= files.keys(), "Missing required license material")
    record.check(any(name.startswith("licenses/crates/") for name in files), "Missing Cargo license material")
    record.check(any(name.startswith("licenses/rust/licenses/") for name in files), "Missing Rust license material")
    record.check(not any(name.lower().endswith(".exe") for name in files), "Executable in license bundle")


def license_bundle(stage, output, records, architecture):
    verify_licenses({path.relative_to(stage).as_posix(): None for path in stage.rglob("*") if path.is_file()})
    # Only portable provenance is delivered; full build records contain local paths.
    builds = []
    for item in records:
        builds.append({key: item[key] for key in (
            "source", "configuration", "rustc", "cargo", "sdk_version", "policy",
            "linker_version", "resource_compiler_version", "executable_sha256",
            "vc_ltl_libraries")})
        if "thunks_sha256" in item:
            builds[-1]["thunks_sha256"] = item["thunks_sha256"]
        builds[-1]["pdb_name"] = Path(item["pdb"]).name
        builds[-1]["pdb_sha256"] = item["pdb_sha256"]
    provenance = {
        "architecture": architecture,
        "builds": builds,
        "cargo_lock_sha256": digest((ROOT / "Cargo.lock").read_bytes()),
        "compatibility_sources": {name: json.loads((ROOT / "inputs" / name).read_text(encoding="utf-8"))
                                  for name in ("vc-ltl-source.json", "yy-thunks-source.json")},
    }
    (stage / "builds").mkdir()
    (stage / "builds" / f"{architecture}.json").write_text(
        json.dumps(provenance, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    record.write_sums(stage)
    record.create_archive(stage, output)
    record.verify_contents(output)


def assemble(inputs, output, configuration="Dist", commit=None):
    record.check(not output.exists(), f"Output already exists: {output}")
    programs = {}
    merged = {}
    identity = None
    policies = json.loads((ROOT / "inputs/targets.json").read_text(encoding="utf-8"))
    for arch in ARCHITECTURES:
        program = inputs / f"winlex-{arch}.zip"
        data = program.read_bytes()
        record.check((inputs / f"{program.name}.sha256").read_text(encoding="utf-8")
                     == f"{digest(data)}  {program.name}\n", f"Program archive hash mismatch: {arch}")
        materials = record.verify_contents(inputs / f"winlex-{arch}-licenses.zip")
        verify_licenses(materials)
        provenance = json.loads(materials[f"builds/{arch}.json"])
        record.check(provenance["architecture"] == arch, "License architecture mismatch")
        builds = provenance["builds"]
        record.check(len(builds) == 2, "Expected CLI and GUI provenance")
        for build in builds:
            record.check(build["configuration"] == configuration, "Delivery configuration mismatch")
            if commit is not None:
                record.check(build["source"] == {"commit": commit, "dirty": False}, "Delivery source mismatch")
            current = {key: build[key] for key in ("source", "configuration", "rustc", "cargo")}
            current["cargo_lock_sha256"] = provenance["cargo_lock_sha256"]
            current["compatibility_sources"] = provenance["compatibility_sources"]
            if identity is None:
                identity = current
            record.check(current == identity, "Build provenance mismatch across delivery")
            record.check(build["policy"] == policies[arch], "Build target policy mismatch")
        files = record.verify_contents(program, check_binary_paths=identity["configuration"] == "Dist")
        symbol_path = inputs / f"winlex-{arch}-symbols.zip"
        symbol_files = record.verify_contents(symbol_path, symbol_archive=True)
        record.check(set(files) == PROGRAM_FILES, f"Unexpected program archive contents: {arch}")
        for build, name, subsystem in zip(builds, ("winlex.exe", "winlex-gui.exe"), (3, 2)):
            record.check(digest(files[name]) == build["executable_sha256"], f"Executable hash mismatch: {arch}/{name}")
            record.verify_pe(files[name], policies[arch], subsystem)
            pdb_name = "winlex.pdb" if name == "winlex.exe" else "winlex_gui.pdb"
            record.check(build["pdb_name"] == pdb_name, "PDB name mismatch")
            pdb = symbol_files[pdb_name]
            record.check(digest(pdb) == build["pdb_sha256"], "PDB hash mismatch")
            record.symbols.verify(files[name], pdb)
        for name in NOTICES:
            record.check(files[name] == materials[name], f"Program/license notice mismatch: {arch}/{name}")
        for name, contents in materials.items():
            record.check(not name.startswith("builds/") or name == f"builds/{arch}.json",
                         "Unexpected build provenance in license bundle")
            record.check(name not in merged or merged[name] == contents, f"Conflicting license material: {name}")
            merged[name] = contents
        programs[program.name] = data
        programs[symbol_path.name] = symbol_path.read_bytes()
    output.parent.mkdir(parents=True, exist_ok=True)
    # Stage the entire delivery before publishing it. Existing outputs are never replaced.
    with tempfile.TemporaryDirectory(prefix="winlex-delivery-", dir=output.parent) as temporary:
        work = Path(temporary)
        stage = work / "licenses"
        stage.mkdir()
        for name, contents in merged.items():
            path = stage / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(contents)
        record.write_sums(stage)
        delivery = work / "delivery"
        delivery.mkdir()
        record.create_archive(stage, delivery / "licenses.zip")
        record.verify_contents(delivery / "licenses.zip")
        for name, contents in programs.items():
            (delivery / name).write_bytes(contents)
        record.write_sums(delivery, "SHA256SUMS")
        shutil.move(str(delivery), str(output))
    try:
        print(f"Verified three-architecture delivery: {output}")
    except UnicodeEncodeError:
        print(f"Verified three-architecture delivery: {str(output)!a}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    licenses = commands.add_parser("licenses")
    licenses.add_argument("--stage", type=Path, required=True)
    licenses.add_argument("--output", type=Path, required=True)
    licenses.add_argument("--architecture", choices=ARCHITECTURES, required=True)
    symbols = commands.add_parser("symbols")
    symbols.add_argument("--output", type=Path, required=True)
    delivery = commands.add_parser("assemble")
    delivery.add_argument("--inputs", type=Path, required=True)
    delivery.add_argument("--output", type=Path, required=True)
    delivery.add_argument("--configuration", choices=("Debug", "Release", "Dist"), default="Dist")
    delivery.add_argument("--commit", help="require clean builds of this source commit")
    args = parser.parse_args()
    if args.command == "licenses":
        import sys
        license_bundle(args.stage, args.output, json.load(sys.stdin.buffer), args.architecture)
    elif args.command == "symbols":
        import sys
        records = json.load(sys.stdin.buffer)
        with tempfile.TemporaryDirectory(prefix="winlex-symbols-") as temporary:
            stage = Path(temporary)
            for item in records:
                pdb = Path(item["pdb"])
                data = pdb.read_bytes()
                record.check(digest(data) == item["pdb_sha256"], "PDB hash mismatch")
                record.symbols.verify(Path(item["executable"]).read_bytes(), data)
                (stage / pdb.name).write_bytes(data)
            record.write_sums(stage)
            record.create_archive(stage, args.output)
            record.verify_contents(args.output, symbol_archive=True)
    else:
        assemble(args.inputs, args.output, args.configuration, args.commit)


if __name__ == "__main__":
    main()
