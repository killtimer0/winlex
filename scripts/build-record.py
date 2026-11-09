"""Record builds and validate executable and archive contents."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import struct
import sys
import zipfile


ROOT = Path(__file__).resolve().parents[1]


def command(*args, cwd=None):
    return subprocess.check_output(args, cwd=cwd, stderr=subprocess.PIPE).decode("utf-8").strip()


def snapshot(root=ROOT):
    try:
        source = {
            "commit": command("git", "rev-parse", "HEAD", cwd=root),
            "dirty": bool(command("git", "status", "--porcelain", "--untracked-files=all", cwd=root)),
        }
    except (OSError, subprocess.CalledProcessError):
        source = None
    return {
        "source": source,
        "rustc": command("rustc", "-Vv"),
        "cargo": command("cargo", "--version"),
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


def verify_records(paths, architecture, configuration):
    target = json.loads((ROOT / "inputs/targets.json").read_text())[architecture]["target"]
    records = [json.loads(path.read_text(encoding="utf-8-sig")) for path in paths]
    for record, name, kind in zip(records, ("winlex.exe", "winlex-gui.exe"), (3, 2)):
        check(record.get("schema_version") == 2, "Unsupported build record; rebuild with scripts/build.ps1")
        policy = record["policy"]
        check(policy["target"] == target and record["configuration"] == configuration,
              "Build record target or configuration mismatch")
        check(policy == records[0]["policy"], "Build record policy mismatch")
        path = Path(record["executable"])
        data = path.read_bytes()
        check(path.name == name and hashlib.sha256(data).hexdigest() == record["executable_sha256"],
              "Executable name or hash mismatch")
        verify_pe(data, policy, kind)
    return records


def verify_contents(path, *, check_binary_paths=False):
    with zipfile.ZipFile(path) as archive:
        names = [info.filename for info in archive.infolist() if not info.is_dir()]
        check(len(names) == len({name.casefold() for name in names}), "Duplicate or case-conflicting archive member")
        check(all(name and not name.startswith(("/", "\\")) and "\\" not in name
                  and ":" not in name and all(part not in ("", ".", "..") for part in name.split("/"))
                  for name in names), "Unsafe archive member")
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
            if not name.lower().endswith(".exe") or check_binary_paths:
                for prefix in (str(ROOT), os.environ.get("USERPROFILE", "")):
                    if prefix:
                        for spelling in {prefix, prefix.replace("\\", "/")}:
                            for encoding in ("utf-8", "utf-16le"):
                                check(spelling.lower().encode(encoding) not in data.lower(),
                                      f"Local build path in archive: {name}")
        return {name: archive.read(name) for name in sums}


def verify_archive(path, records):
    files = verify_contents(path, check_binary_paths=records[0].get("configuration") == "Dist")
    for record in records:
        name = Path(record["executable"]).name
        check(hashlib.sha256(files[name]).hexdigest() == record["executable_sha256"],
              f"Archive executable mismatch: {name}")


def write_sums(directory, name="SHA256SUMS.txt"):
    files = sorted(path for path in directory.rglob("*") if path.is_file() and path != directory / name)
    (directory / name).write_text("".join(
        f"{hashlib.sha256(path.read_bytes()).hexdigest()}  {path.relative_to(directory).as_posix()}\n" for path in files),
        encoding="utf-8")


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
    parser.add_argument("--stage", type=Path, help="archive a prepared directory using validated records from stdin")
    parser.add_argument("--json", action="store_true", help="emit validated records as JSON")
    args = parser.parse_args()
    if args.stage:
        if not args.archive or args.records:
            parser.error("--stage requires --archive and validated records on stdin, without --records")
        records = json.load(sys.stdin)
        write_sums(args.stage)
        create_archive(args.stage, args.archive)
        verify_archive(args.archive, records)
        print("PASS archive contents and checksums")
        return
    if args.records:
        if not args.architecture:
            parser.error("--records requires --architecture")
        records = verify_records(args.records, args.architecture, args.configuration)
        if args.archive:
            verify_archive(args.archive, records)
        if args.json:
            print(json.dumps(records))
        else:
            print("PASS build records and PE/CRT contracts" + ("; archive checksums" if args.archive else ""))
    else:
        if args.archive:
            parser.error("--archive requires --records")
        print(json.dumps(snapshot()))


if __name__ == "__main__":
    main()
