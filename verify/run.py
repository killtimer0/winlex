"""Run query tests and independent reader checks; optionally check SDK ABI."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / "tmp/verify"


def run(command):
    p = subprocess.run(list(map(str, command)), cwd=ROOT, capture_output=True)
    if p.returncode:
        sys.stderr.buffer.write(p.stdout + p.stderr)
        raise SystemExit(p.returncode)
    sys.stdout.buffer.write(p.stdout)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--abi", action="store_true", help="also run compiler_gate.py (see docs/build.md)")
    parser.add_argument("--winrt-sdk", help="Windows Kits/10 root for real WinRT metadata checks")
    parser.add_argument("--winrt-version", default="10.0.26100.0")
    parser.add_argument("--winrt-apps", help="directory containing extracted App SDK packages")
    args = parser.parse_args()
    OUT.mkdir(parents=True, exist_ok=True)
    source = json.loads((ROOT / "inputs/source.json").read_text())
    winmd = ROOT / "inputs" / source["file"]
    assert hashlib.sha256(winmd.read_bytes()).hexdigest() == source["sha256"]
    run(["cargo", "build", "-p", "winlex-core", "--example", "reader-facts", "--release", "--locked"])
    run(["cargo", "build", "-p", "winlex-cli", "--example", "host-cli", "--release", "--locked"])
    run(["cargo", "test", "-p", "winlex-cli", "-p", "winlex-core", "--release", "--locked"])
    if args.abi:
        run([sys.executable, "verify/compiler_gate.py"])
    run([sys.executable, "verify/reader_gate.py"])
    run([sys.executable, "verify/fixture_gate.py"])
    run([sys.executable, "verify/multisource_gate.py"])
    if args.winrt_sdk:
        run(["cargo", "build", "-p", "winlex-core", "--example", "query-audit", "--release", "--locked"])
        command = [sys.executable, "verify/winrt_gate.py", "--sdk", args.winrt_sdk, "--version", args.winrt_version]
        if args.winrt_apps:
            command.extend(["--apps", args.winrt_apps])
        run(command)


if __name__ == "__main__":
    main()
