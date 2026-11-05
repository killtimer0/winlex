"""Compare every local WinMD in a directory with System.Reflection.Metadata."""
import argparse
from datetime import datetime, timezone
import hashlib
import json
import math
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[2]
COVERAGE = {
    "semantic": ["assembly identity and references", "types, flags and member ownership",
                 "field, method and property signature trees", "parameter sequences and generic parameters",
                 "constant types and values (floating-point bits)", "class and field layouts",
                 "properties, events and accessors", "P/Invoke imports", "type reference scopes",
                 "selected Win32 and WinRT attribute argument values"],
    "raw": ["custom attribute parent, constructor and value blob", "interface and nesting associations"],
    "not_compared": ["unselected custom attribute argument decoding and named field/property distinction",
                     "IsConst modifier required/optional distinction",
                     "generic constraints", "marshalling descriptors",
                     "method bodies, resources and debug information", "CLI query and declaration formatting"],
    "normalization": ["generic parameter identity uses scope and index, not display name",
                      "omitted array lower bounds are zero", "floating-point values use IEEE bit patterns",
                      "IsConst maps to vendor const qualification (including byref)"],
}


def write_json(path, value):
    path.write_text(json.dumps(value, ensure_ascii=True, indent=2) + "\n", encoding="utf-8")


def discover(directory):
    if not directory.is_dir():
        raise ValueError(f"Input directory does not exist: {directory}")
    files = sorted((p for p in directory.rglob("*") if p.is_file() and p.suffix.lower() == ".winmd"),
                   key=lambda p: p.relative_to(directory).as_posix())
    if not files:
        raise ValueError(f"No WinMD files in: {directory}")
    return files


def differences(actual, expected, path="$/"):
    if type(actual) is not type(expected):
        yield {"path": path, "rust": actual, "dotnet": expected}
    elif isinstance(actual, dict):
        for key in sorted(actual.keys() | expected.keys()):
            escaped = key.replace("~", "~0").replace("/", "~1")
            child = path + escaped
            if key not in actual or key not in expected:
                yield {"path": child, "rust": actual.get(key), "dotnet": expected.get(key),
                       "missing_in": "rust" if key not in actual else "dotnet"}
            else:
                yield from differences(actual[key], expected[key], child + "/")
    elif isinstance(actual, list):
        for i in range(max(len(actual), len(expected))):
            if i >= len(actual) or i >= len(expected):
                yield {"path": path + str(i), "rust": actual[i] if i < len(actual) else None,
                       "dotnet": expected[i] if i < len(expected) else None,
                       "missing_in": "rust" if i >= len(actual) else "dotnet"}
            else:
                yield from differences(actual[i], expected[i], path + str(i) + "/")
    elif actual != expected:
        yield {"path": path, "rust": actual, "dotnet": expected}


def extract(command, source, output, side, timeout):
    destination = output / f"{side}.json"
    args = [*map(str, command), str(source)]
    if side == "dotnet":
        args.append(str(destination))
    with (output / f"{side}.stdout").open("wb") as stdout, (output / f"{side}.stderr").open("wb") as stderr:
        result = subprocess.run(args, stdout=stdout, stderr=stderr, timeout=timeout, cwd=ROOT)
    if result.returncode:
        raise RuntimeError(f"{side} exited with {result.returncode}; see {side}.stderr")
    if side == "rust":
        destination.write_bytes((output / "rust.stdout").read_bytes())
    facts = json.loads(destination.read_text(encoding="utf-8"))
    required = {"assembly", "references", "types", "scopes", "fields", "methods", "parameters", "generics",
                "constants", "properties", "events", "attributes_raw", "attributes_decoded", "layouts", "interfaces", "nested"}
    if not isinstance(facts, dict) or facts.keys() != required:
        raise ValueError(f"{side} returned an invalid facts schema")
    return facts


def compare_files(directory, files, output, rust, dotnet, timeout):
    records = []
    for index, source in enumerate(files):
        work = output / f"{index:04d}"
        work.mkdir()
        record = {"file": source.relative_to(directory).as_posix(), "evidence": work.name,
                  "passed": False, "errors": {}}
        facts = {}
        try:
            record["sha256"] = hashlib.sha256(source.read_bytes()).hexdigest()
            for side, command in (("rust", rust), ("dotnet", dotnet)):
                try:
                    facts[side] = extract(command, source, work, side, timeout)
                except (OSError, ValueError, RuntimeError, subprocess.SubprocessError) as error:
                    record["errors"][side] = str(error)
            if len(facts) == 2:
                delta = list(differences(facts["rust"], facts["dotnet"]))
                write_json(work / "differences.json", delta)
                record["differences"] = len(delta)
                record["counts"] = {k: len(v) for k, v in facts["rust"].items() if isinstance(v, (dict, list))}
                record["passed"] = not delta
            if hashlib.sha256(source.read_bytes()).hexdigest() != record["sha256"]:
                record["errors"]["input"] = "Input changed during comparison"
                record["passed"] = False
        except OSError as error:
            record["errors"]["input"] = str(error)
            record["passed"] = False
        records.append(record)
        write_json(work / "result.json", record)
        print(f"{'PASS' if record['passed'] else 'FAIL'} {record['file']}", flush=True)
    report = {"passed": bool(records) and all(r["passed"] for r in records), "input": str(directory),
              "coverage": COVERAGE, "files": records}
    write_json(output / "report.json", report)
    return report


def build(output, args):
    options = []
    if args.target:
        options += ["--target", args.target]
    if args.target_dir:
        options += ["--target-dir", str(args.target_dir)]
    command = ["cargo", "build", "-p", "winlex-format", "--example", "winmd-facts", "--locked",
               "--message-format=json-render-diagnostics", *options]
    with (output / "cargo.jsonl").open("wb") as log, (output / "cargo.stderr").open("wb") as err:
        result = subprocess.run(command, cwd=ROOT, stdout=log, stderr=err)
    result.check_returncode()
    messages = [json.loads(line) for line in (output / "cargo.jsonl").read_text(encoding="utf-8").splitlines() if line.strip()]
    paths = {m["executable"] for m in messages if m.get("reason") == "compiler-artifact"
             and m.get("target", {}).get("name") == "winmd-facts" and m.get("executable")}
    if len(paths) != 1:
        raise RuntimeError("Cargo did not report exactly one winmd-facts executable")
    oracle = output / "oracle"
    with (output / "dotnet-build.log").open("wb") as log:
        subprocess.run(["dotnet", "build", ROOT / "tests/differential/Oracle", "-c", "Debug", "-o", oracle],
                       cwd=ROOT, stdout=log, stderr=subprocess.STDOUT, check=True)
    return [paths.pop()], ["dotnet", oracle / "Oracle.dll"]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory", type=Path)
    parser.add_argument("--output", type=Path, help="new evidence directory (must not already exist)")
    parser.add_argument("--target", help="host-runnable Rust target")
    parser.add_argument("--target-dir", type=Path)
    parser.add_argument("--timeout", type=float, default=180, help="seconds per reader per file")
    args = parser.parse_args()
    directory = args.directory.absolute()
    output = (args.output or ROOT / "tmp/differential" / datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%S%fZ")).absolute()
    try:
        if not math.isfinite(args.timeout) or args.timeout <= 0:
            raise ValueError("Timeout must be finite and positive")
        files = discover(directory)
        output.mkdir(parents=True, exist_ok=False)
        rust, dotnet = build(output, args)
        report = compare_files(directory, files, output, rust, dotnet, args.timeout)
        print(f"Report: {output / 'report.json'}")
        return 0 if report["passed"] else 1
    except (OSError, ValueError, RuntimeError, subprocess.SubprocessError) as error:
        print(f"Comparison failed: {error}\nEvidence: {output}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
