"""Verify native GUID layouts are independent of an optional CLR reference assembly."""
import argparse
import json
from pathlib import Path
import subprocess

from artifacts import executable

ROOT = Path(__file__).resolve().parents[2]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--inputs", type=Path, default=ROOT / "inputs/Windows.Win32.winmd",
                        help="a WinMD file or a directory containing the WinMD collection")
    parser.add_argument("--reference", type=Path, required=True,
                        help="original NETStandard.Library.Ref 2.1.0 netstandard.dll")
    args = parser.parse_args()
    inputs = sorted(args.inputs.glob("*.winmd")) if args.inputs.is_dir() else [args.inputs]
    assert inputs and args.reference.is_file()

    def layout(files, name, arch):
        result = subprocess.run([str(executable("host-cli")), *map(str, files), "--json",
                                 "--arch", arch, "layout", name],
                                capture_output=True, check=True, encoding="utf-8")
        return json.loads(result.stdout)["result"]

    checks = 0
    for arch in ("x86", "x64", "arm64"):
        for name in ("Windows.Win32.System.Com.TYPEATTR", "Windows.Win32.System.Com.TLIBATTR"):
            before = layout(inputs, name, arch)
            after = layout([*inputs, args.reference], name, arch)
            assert before == after, (name, arch, before, after)
            assert next(f for f in before["fields"] if f["name"] == "guid")["size_bits"] == 128
            checks += 1
        guid = layout([*inputs, args.reference], "System.Guid", arch)
        assert (guid["size_bits"], guid["alignment_bits"], guid["fields"]) == (128, 32, [])
        checks += 1
    print(f"Passed {checks} GUID/reference-independence checks across x86/x64/arm64")


if __name__ == "__main__":
    main()
