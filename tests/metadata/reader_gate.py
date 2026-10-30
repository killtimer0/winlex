"""Compare reader additions with independent ECMA-335 decoding in .NET."""
import json
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parents[2]


def compare(actual, expected):
    counts = {}
    for key in ("assembly", "references", "layouts", "scopes", "signature_type_tokens", "properties", "events"):
        left, right = actual[key], expected[key]
        if key in ("properties", "events"):
            def ordered(rows):
                return [{**row, "accessors": sorted(row["accessors"], key=lambda a: (a["method"], a["flags"]))}
                        for row in rows]
            left, right = ordered(left), ordered(right)
        assert left == right, f"independent mismatch: {key}"
        counts[key] = len(left)
    return counts


def main():
    out = ROOT / "tmp/verify"
    out.mkdir(parents=True, exist_ok=True)
    subprocess.run(["dotnet", "run", "--project", str(ROOT / "tests/metadata/MetadataOracle"), "-c", "Release", "--", str(ROOT / "inputs/Windows.Win32.winmd"), str(out / "oracle.json")], check=True)
    p = subprocess.run([str(ROOT / "target/release/examples/reader-facts.exe"), str(ROOT / "inputs/Windows.Win32.winmd")], capture_output=True, check=True)
    (out / "reader-facts.json").write_bytes(p.stdout)
    actual = json.loads(p.stdout)
    oracle = json.loads((out / "oracle.json").read_text())
    counts = compare(actual, oracle)
    result = {"passed": True, "compared": counts, "scope": "Named type tokens in field/method signatures; modifiers and TypeSpec expansion are not a lossless AST guarantee"}
    (out / "reader-gate.json").write_text(json.dumps(result, indent=2))
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
