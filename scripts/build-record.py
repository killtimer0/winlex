"""Capture source and toolchain identities for build and packaging contracts."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import struct
import zipfile


ROOT = Path(__file__).resolve().parents[1]


def command(*args, cwd=None):
    return subprocess.check_output(args, cwd=cwd).decode("utf-8").strip()


def tree_hash(root, names):
    digest = hashlib.sha256()
    for name in sorted(set(names)):
        path = root / name
        data = (os.readlink(path).encode("utf-8") if path.is_symlink() else
                path.read_bytes() if path.is_file() else b"<missing>")
        digest.update(name.encode("utf-8") + b"\0")
        digest.update(hashlib.sha256(data).digest())
    return digest.hexdigest()


def snapshot(root=ROOT):
    names = subprocess.check_output(
        ["git", "ls-files", "--cached", "--others", "--exclude-standard", "-z"], cwd=root
    ).decode("utf-8").rstrip("\0").split("\0")
    source = {
        "commit": command("git", "rev-parse", "HEAD", cwd=root),
        "dirty": bool(command("git", "status", "--porcelain", "--untracked-files=all", cwd=root)),
        "sha256": tree_hash(root, names),
    }
    sysroot = Path(command("rustc", "--print", "sysroot"))
    docs = sysroot / "share/doc/rust"
    licenses = ["COPYRIGHT-library.html", *(
        path.relative_to(docs).as_posix() for path in (docs / "licenses").rglob("*") if path.is_file()
    )]
    license_hash = (tree_hash(docs, licenses)
                    if (docs / "COPYRIGHT-library.html").is_file() and len(licenses) > 1 else None)
    return {
        "source": source,
        "rustc": command("rustc", "-Vv"),
        "cargo": command("cargo", "--version"),
        "rust_licenses_sha256": license_hash,
    }


def check(condition, message):
    if not condition:
        raise ValueError(message)


def verify_pe(data, policy, subsystem):
    check(data[:2] == b"MZ", "Missing DOS header")
    pe = struct.unpack_from("<I", data, 0x3c)[0]
    check(data[pe:pe + 4] == b"PE\0\0", "Missing PE signature")
    machine, sections = struct.unpack_from("<HH", data, pe + 4)
    check(machine == policy["machine"], "PE machine mismatch")
    optional = pe + 24
    magic = struct.unpack_from("<H", data, optional)[0]
    check(magic in (0x10b, 0x20b), "Unsupported PE optional header")
    version = struct.unpack_from("<HH", data, optional + 48)
    check(version == tuple(map(int, policy["subsystem"].split("."))), "PE subsystem version mismatch")
    check(struct.unpack_from("<H", data, optional + 68)[0] == subsystem, "PE subsystem kind mismatch")
    section_table = optional + struct.unpack_from("<H", data, pe + 20)[0]

    def offset(rva):
        if rva < struct.unpack_from("<I", data, optional + 60)[0]:
            return rva
        for index in range(sections):
            size, address, raw_size, raw = struct.unpack_from("<IIII", data, section_table + index * 40 + 8)
            if address <= rva < address + max(size, raw_size):
                check(rva - address < raw_size, "PE RVA has no file data")
                return raw + rva - address
        raise ValueError("Invalid PE RVA")

    directories = optional + (112 if magic == 0x20b else 96)
    imports = set()
    for directory, stride in ((1, 20), (13, 32)):
        rva, size = struct.unpack_from("<II", data, directories + directory * 8)
        if not rva:
            continue
        descriptor = offset(rva)
        for index in range(size // stride):
            entry = data[descriptor + index * stride:descriptor + (index + 1) * stride]
            check(len(entry) == stride, "Truncated PE import descriptor")
            if not any(entry):
                break
            name = struct.unpack_from("<I", entry, 12 if directory == 1 else 4)[0]
            if directory == 13 and not (struct.unpack_from("<I", entry)[0] & 1):
                image_base = struct.unpack_from("<Q" if magic == 0x20b else "<I", data,
                                                optional + (24 if magic == 0x20b else 28))[0]
                name -= image_base
            start = offset(name)
            end = data.index(b"\0", start)
            imports.add(data[start:end].decode("ascii").lower())
        else:
            raise ValueError("Unterminated PE import directory")
    check(policy["crt"] in imports, "Required system CRT import missing")
    for dll in imports:
        runtime = dll.startswith(("vcruntime", "msvcp", "concrt", "msvcr", "ucrtbase"))
        check(not runtime or dll == policy["crt"], f"Unexpected CRT import: {dll}")
    return sorted(imports)


def verify_records(paths, architecture, configuration):
    policy = json.loads((ROOT / "inputs/targets.json").read_text())[architecture]
    current = snapshot()
    ltl = json.loads((ROOT / "inputs/vc-ltl-source.json").read_text(encoding="utf-8-sig"))
    thunks = json.loads((ROOT / "inputs/yy-thunks-source.json").read_text(encoding="utf-8-sig"))
    records = [json.loads(path.read_text(encoding="utf-8-sig")) for path in paths]
    check(len(records) == 2, "Two build records are required")
    check(records[0].get("build_id") and records[0].get("build_id") == records[1].get("build_id"),
          "Build invocation mismatch")
    for record, name, kind in zip(records, ("winlex.exe", "winlex-gui.exe"), (3, 2)):
        check(record.get("schema_version") == 1, "Unsupported build record; rebuild with scripts/build.ps1")
        check(record.get("build_id") and record["build_id"] == records[0].get("build_id"), "Build invocation mismatch")
        check(record["target"] == policy["target"] and record["configuration"] == configuration,
              "Build record target or configuration mismatch")
        check(record["policy"] == policy and record["minimum_windows"] == policy["minimum_windows"],
              "Build record policy mismatch")
        for field in ("source", "rustc", "cargo", "rust_licenses_sha256"):
            check(record.get(field) == current[field], f"Build provenance mismatch: {field}")
        check(current["rust_licenses_sha256"], "Rust license documentation missing; install rust-docs and rebuild")
        check(configuration != "Dist" or not record["source"]["dirty"], "Dist packaging requires a clean source tree")
        check(record["crt"] == f"VC-LTL5 system {policy['crt_mode']}", "Build record CRT mismatch")
        check(record["vc_ltl_libraries"] == ltl["libraries"][policy["vc_ltl_platform"]], "Build record library mismatch")
        if policy["thunks_arch"]:
            check(record.get("thunks_sha256") == thunks["objects"][policy["thunks_arch"]], "Build record thunk mismatch")
        else:
            check("thunks_object" not in record and "thunks_sha256" not in record, "Unexpected thunk input")
        path = Path(record["executable"])
        data = path.read_bytes()
        check(path.name == name and hashlib.sha256(data).hexdigest() == record["executable_sha256"],
              "Executable name or hash mismatch")
        verify_pe(data, policy, kind)
    return records


def verify_archive(path, records):
    with zipfile.ZipFile(path) as archive:
        names = [info.filename for info in archive.infolist() if not info.is_dir()]
        check(len(names) == len(set(names)), "Duplicate archive member")
        check(not any(name.lower().endswith((".dll", ".winmd", ".build.json")) for name in names),
              "Unexpected runtime, metadata or build record in archive")
        sums = {}
        for line in archive.read("SHA256SUMS.txt").decode("utf-8").splitlines():
            digest, name = line.split("  ", 1)
            check(name not in sums, "Duplicate checksum entry")
            sums[name] = digest
        check(set(sums) == set(names) - {"SHA256SUMS.txt"}, "Archive checksum set mismatch")
        for name, digest in sums.items():
            data = archive.read(name)
            check(hashlib.sha256(data).hexdigest() == digest, f"Archive hash mismatch: {name}")
            if not name.lower().endswith(".exe") or records[0].get("configuration") == "Dist":
                for prefix in (str(ROOT), os.environ.get("USERPROFILE", "")):
                    if prefix:
                        for spelling in {prefix, prefix.replace("\\", "/")}:
                            for encoding in ("utf-8", "utf-16le"):
                                check(spelling.lower().encode(encoding) not in data.lower(),
                                      f"Local build path in archive: {name}")
        for record in records:
            name = Path(record["executable"]).name
            check(hashlib.sha256(archive.read(name)).hexdigest() == record["executable_sha256"],
                  f"Archive executable mismatch: {name}")


def create_archive(stage, path):
    with zipfile.ZipFile(path, "x", compression=zipfile.ZIP_DEFLATED, strict_timestamps=False) as archive:
        for member in sorted(stage.rglob("*")):
            if member.is_file():
                archive.write(member, member.relative_to(stage).as_posix())


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--records", type=Path, nargs=2)
    parser.add_argument("--architecture", choices=("x86", "x64", "arm64"))
    parser.add_argument("--configuration", choices=("Debug", "Release", "Dist"), default="Dist")
    parser.add_argument("--archive", type=Path)
    parser.add_argument("--stage", type=Path, help="create the archive from a prepared directory before validation")
    parser.add_argument("--json", action="store_true", help="emit validated records as JSON")
    args = parser.parse_args()
    if args.stage and (not args.archive or not args.records):
        parser.error("--stage requires --archive and --records")
    if args.records:
        if not args.architecture:
            parser.error("--records requires --architecture")
        records = verify_records(args.records, args.architecture, args.configuration)
        if args.archive:
            if args.stage:
                create_archive(args.stage, args.archive)
            verify_archive(args.archive, records)
        if args.json:
            print(json.dumps(records))
        else:
            print("PASS build provenance, target policy and PE/CRT contracts" + ("; archive checksums" if args.archive else ""))
    else:
        if args.archive:
            parser.error("--archive requires --records")
        print(json.dumps(snapshot()))


if __name__ == "__main__":
    main()
