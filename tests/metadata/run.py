"""Run query tests and independent reader checks; optionally check SDK ABI."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
from artifacts import build_example, manifest_path, write_manifest

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / "tmp/verify"


def run(command):
    p = subprocess.run(list(map(str, command)), cwd=ROOT)
    if p.returncode:
        raise SystemExit(p.returncode)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--abi", action="store_true", help="also run tests/abi/run.py (see docs/build.md)")
    parser.add_argument("--winrt-sdk", help="Windows Kits/10 root for real WinRT metadata checks")
    parser.add_argument("--winrt-version", default="10.0.26100.0")
    parser.add_argument("--winrt-apps", help="directory containing extracted App SDK packages")
    parser.add_argument("--target-dir", help="Cargo output directory (otherwise honor Cargo configuration)")
    parser.add_argument("--target", help="Cargo target (otherwise honor Cargo configuration)")
    parser.add_argument("--profile", default="dev", help="Cargo profile (default: dev/Debug)")
    parser.add_argument("--artifacts", type=Path, default=manifest_path(), help="persist executable paths for standalone gates")
    args = parser.parse_args()
    OUT.mkdir(parents=True, exist_ok=True)
    source = json.loads((ROOT / "inputs/source.json").read_text())
    winmd = ROOT / "inputs" / source["file"]
    assert hashlib.sha256(winmd.read_bytes()).hexdigest() == source["sha256"]
    options = ["--profile", args.profile]
    if args.target_dir:
        options.extend(["--target-dir", args.target_dir])
    if args.target:
        options.extend(["--target", args.target])
    executables = {
        "reader-facts": build_example("winlex-format", "reader-facts", options),
        "host-cli": build_example("winlex-cli", "host-cli", options),
    }
    if args.winrt_sdk:
        executables["query-audit"] = build_example("winlex-format", "query-audit", options)
    write_manifest(executables, args.artifacts.absolute())
    os.environ["WINLEX_GATE_ARTIFACTS"] = str(args.artifacts.absolute())
    run(["cargo", "test", "-p", "winlex-cli", "-p", "winlex-core", "--locked", *options])
    if args.abi:
        run([sys.executable, "tests/abi/run.py"])
    run([sys.executable, "tests/metadata/reader_gate.py"])
    run([sys.executable, "tests/metadata/fixture_gate.py"])
    run([sys.executable, "tests/metadata/multisource_gate.py"])
    if args.winrt_sdk:
        command = [sys.executable, "tests/metadata/winrt_gate.py", "--sdk", args.winrt_sdk, "--version", args.winrt_version]
        if args.winrt_apps:
            command.extend(["--apps", args.winrt_apps])
        run(command)


if __name__ == "__main__":
    main()
