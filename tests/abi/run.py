"""Independent SDK ABI checks; SDK is test-only, never consumed by query/index code."""
import json
import os
from pathlib import Path
import subprocess
import struct
import shutil
import sys

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tests/metadata"))
from artifacts import executable
WINMD = ROOT / "inputs/Windows.Win32.winmd"
SDK = Path(os.environ["WINLEX_SDK_INCLUDE"])
MSVC = Path(os.environ["WINLEX_MSVC_ROOT"])
CLANG = os.environ.get("WINLEX_CLANG") or shutil.which("clang")
OUT = ROOT / "tmp/verify/compiler"


def query(command, name, arch):
    cmd = [executable("host-cli"), WINMD, "--json", "--arch", arch, command, name]
    p = subprocess.run(cmd, capture_output=True, text=True, encoding="utf-8", check=True)
    return json.loads(p.stdout)["result"]


def symbol_bytes(path, wanted, size):
    data = path.read_bytes()
    _, sections, _, symptr, symcount, optsize, _ = struct.unpack_from("<HHIIIHH", data)
    strings = symptr + symcount * 18
    index = 0
    while index < symcount:
        offset = symptr + index * 18
        raw = data[offset:offset + 8]
        if raw[:4] == b"\0" * 4:
            start = strings + struct.unpack_from("<I", raw, 4)[0]
            name = data[start:data.index(b"\0", start)].decode()
        else:
            name = raw.rstrip(b"\0").decode()
        value, section, _, _, aux = struct.unpack_from("<IhHBB", data, offset + 8)
        if name in (wanted, "_" + wanted):
            assert 1 <= section <= sections
            header = 20 + optsize + (section - 1) * 40
            rawsize, rawptr = struct.unpack_from("<II", data, header + 16)
            assert value + size <= rawsize
            return data[rawptr + value:rawptr + value + size]
        index += 1 + aux
    raise AssertionError(f"Missing COFF symbol {wanted}")


def main():
    OUT.mkdir(parents=True, exist_ok=True)
    if not CLANG:
        raise SystemExit("Set WINLEX_CLANG or add clang to PATH (required for all three architectures)")
    reports = []
    for arch in ("x86", "x64", "arm64"):
        source = ["#define CINTERFACE", "#include <windows.h>", "#include <commdlg.h>", "#include <objidl.h>", "#include <stddef.h>"]
        count = 0

        def check(expr, expected):
            nonlocal count
            source.append(f'_Static_assert(({expr}) == {expected}, "check{count}");')
            count += 1

        layouts = []
        for name in ("OPENFILENAMEW", "OPENFILENAMEA", "POINT", "RECT", "FILETIME", "SECURITY_ATTRIBUTES", "WIN32_FIND_DATAW", "OVERLAPPED", "DCB"):
            layout = query("layout", name, arch)
            layouts.append(layout)
            check(f"sizeof({name}) * 8", layout["size_bits"])
            check(f"__alignof({name}) * 8", layout["alignment_bits"])
            for field in layout["fields"]:
                if field.get("bitfields"):
                    continue  # C cannot apply offsetof to bitfields; verify COFF bytes below.
                if name == "OVERLAPPED" and field["name"] == "Anonymous":
                    # SDK exposes these union/struct members anonymously. This is a
                    # test-side source spelling map, not a layout recipe.
                    def promoted(fields, offset):
                        for child in fields:
                            absolute = offset + child["offset_bits"]
                            if child["name"] == "Anonymous":
                                promoted(child["fields"], absolute)
                            else:
                                check(f"offsetof(OVERLAPPED, {child['name']}) * 8", absolute)
                    promoted(field["fields"], field["offset_bits"])
                    continue
                check(f"offsetof({name}, {field['name']}) * 8", field["offset_bits"])
        vtables = []
        for name in ("IUnknown", "ISequentialStream", "IStream"):
            vtable = query("vtable", name, arch)
            vtables.append(vtable)
            for slot in vtable["slots"]:
                check(f"offsetof({name}Vtbl, {slot['method']}) / sizeof(void*)", slot["index"])
        assignments = {"fBinary": 1, "fDtrControl": 3, "fRtsControl": 2, "fDummy2": 0x15555}
        source.append("const DCB wl_dcb_bits = {" + ", ".join(f".{name}={value}" for name, value in assignments.items()) + "};")
        path = OUT / f"{arch}.c"
        path.write_text("\n".join(source) + "\n")
        cl = MSVC / "bin/Hostx64" / arch / "cl.exe"
        env = os.environ.copy()
        env["PATH"] = str(cl.parent) + os.pathsep + env.get("PATH", "")
        includes = [MSVC / "include", *[SDK / x for x in ("ucrt", "shared", "um", "winrt")]]
        commands = {
            "msvc": [str(cl), "/nologo", "/TC", "/std:c11", "/c", str(path), f"/Fo{OUT / (arch + '.obj')}", *["/I" + str(p) for p in includes]],
            "clang": [str(CLANG), "--target=" + {"x86": "i686", "x64": "x86_64", "arm64": "aarch64"}[arch] + "-pc-windows-msvc", "-std=c11", "-fms-extensions", "-fms-compatibility", "-fsyntax-only", str(path), *["-I" + str(p) for p in includes]],
        }
        results = {}
        for name, command in commands.items():
            if name == "msvc" and not cl.exists():
                results[name] = {"skipped": "MSVC target compiler not installed"}
                continue
            p = subprocess.run(command, env=env, capture_output=True, timeout=90)
            (OUT / f"{arch}-{name}.log").write_bytes(p.stdout + p.stderr)
            results[name] = {"command": command, "returncode": p.returncode}
            if name == "msvc" and p.returncode == 0:
                dcb = next(l for l in layouts if l["name"] == "DCB")
                expected = 0
                for field in dcb["fields"]:
                    for bit in field.get("bitfields", []):
                        expected |= assignments.get(bit["name"], 0) << bit["offset_bits"]
                size = dcb["size_bits"] // 8
                assert symbol_bytes(OUT / (arch + ".obj"), "wl_dcb_bits", size) == expected.to_bytes(size, "little")
                results[name]["bitfield_bytes_match"] = True
        reports.append({"arch": arch, "assertions": count, "compilers": results, "layouts": layouts, "vtables": vtables})
        (OUT / "report.json").write_text(json.dumps(reports, indent=2))
        assert all(r.get("returncode", 0) == 0 for r in results.values()), f"See {OUT}/{arch}-*.log"
    summary = {"architectures": 3, "assertions": {c: sum(r["assertions"] for r in reports if r["compilers"][c].get("returncode") == 0) for c in ("msvc", "clang")}, "overlapped_promoted_members_checked": True, "msvc_bitfield_byte_checks": sum(r["compilers"]["msvc"].get("bitfield_bytes_match", False) for r in reports), "entrypoint": str(executable("host-cli"))}
    (OUT / "summary.json").write_text(json.dumps(summary, indent=2))
    print(json.dumps(summary, indent=2))


if __name__ == "__main__":
    main()
