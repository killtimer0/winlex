"""Run query and fixture checks; optionally check SDK ABI."""
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


def run(command, timeout=600):
    try:
        p = subprocess.run(list(map(str, command)), cwd=ROOT, timeout=timeout)
    except (OSError, subprocess.TimeoutExpired) as error:
        print(f"FAIL: {command[0]}: {error}", file=sys.stderr)
        return False
    if p.returncode:
        print(f"FAIL: {command[0]} exited with {p.returncode}", file=sys.stderr)
    return p.returncode == 0


def positive_seconds(value):
    seconds = float(value)
    if not 0 < seconds < float("inf"):
        raise argparse.ArgumentTypeError("timeout must be finite and positive")
    return seconds


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--abi", action="store_true", help="also run tests/abi/run.py (see docs/build.md)")
    parser.add_argument("--winrt-sdk", help="Windows Kits/10 root for real WinRT metadata checks")
    parser.add_argument("--winrt-version", default="10.0.26100.0")
    parser.add_argument("--winrt-apps", help="directory containing extracted App SDK packages")
    parser.add_argument("--target-dir", help="Cargo output directory (otherwise honor Cargo configuration)")
    parser.add_argument("--target", help="Cargo target (otherwise honor Cargo configuration)")
    parser.add_argument("--profile", default="dev", help="Cargo profile (default: dev/Debug)")
    parser.add_argument("--build-timeout", type=positive_seconds, default=300,
                        help="seconds per Cargo build/test subprocess (default: 300)")
    parser.add_argument("--gate-timeout", type=positive_seconds, default=600,
                        help="fallback seconds per complete gate, not per query (default: 600; increase for large SDK runs)")
    parser.add_argument("--artifacts", type=Path, default=manifest_path(), help="persist executable paths for standalone gates")
    args = parser.parse_args()
    if args.winrt_apps and not args.winrt_sdk:
        parser.error("--winrt-apps requires --winrt-sdk")
    for option, directory in (("--winrt-sdk", args.winrt_sdk), ("--winrt-apps", args.winrt_apps)):
        if directory and not Path(directory).is_dir():
            parser.error(f"{option} directory does not exist: {directory}")
    OUT.mkdir(parents=True, exist_ok=True)
    source = json.loads((Path(__file__).parent / "source.json").read_text())
    winmd = ROOT / "inputs" / source["file"]
    assert hashlib.sha256(winmd.read_bytes()).hexdigest() == source["sha256"]
    options = ["--profile", args.profile]
    if args.target_dir:
        options.extend(["--target-dir", args.target_dir])
    if args.target:
        options.extend(["--target", args.target])
    executables = {}
    failed = []
    skipped = []
    # Invalidate an earlier run's artifacts before attempting any builds.
    write_manifest(executables, args.artifacts.absolute())
    builds = [("winlex-cli", "host-cli")]
    if args.winrt_sdk:
        builds.append(("winlex-format", "query-audit"))
    for package, name in builds:
        try:
            executables[name] = build_example(package, name, options, timeout=args.build_timeout)
        except (OSError, RuntimeError, ValueError, subprocess.SubprocessError) as error:
            print(f"FAIL: build {name}: {error}", file=sys.stderr)
            failed.append(f"build {name}")
    write_manifest(executables, args.artifacts.absolute())
    os.environ["WINLEX_GATE_ARTIFACTS"] = str(args.artifacts.absolute())
    phases = [("cargo tests", ["cargo", "test", "-p", "winlex-cli", "-p", "winlex-core", "--locked", *options], (), args.build_timeout)]
    if args.abi:
        phases.append(("ABI", [sys.executable, "tests/abi/run.py"], ("host-cli",), args.gate_timeout))
    for name in ("fixture", "multisource"):
        phases.append((name, [sys.executable, f"tests/metadata/{name}_gate.py"], ("host-cli",), args.gate_timeout))
    if args.winrt_sdk:
        command = [sys.executable, "tests/metadata/winrt_gate.py", "--sdk", args.winrt_sdk, "--version", args.winrt_version]
        if args.winrt_apps:
            command.extend(["--apps", args.winrt_apps])
        phases.append(("WinRT", command, ("host-cli", "query-audit"), args.gate_timeout))
    for name, command, required, timeout in phases:
        missing = [item for item in required if item not in executables]
        if missing:
            print(f"SKIP: {name}: build unavailable: {', '.join(missing)}", file=sys.stderr)
            skipped.append(name)
        elif not run(command, timeout=timeout):
            failed.append(name)
    if failed or skipped:
        print(f"Metadata checks: failed={', '.join(failed) or 'none'}; "
              f"skipped={', '.join(skipped) or 'none'}", file=sys.stderr)
    return int(bool(failed or skipped))


if __name__ == "__main__":
    sys.exit(main())
