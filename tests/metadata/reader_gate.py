"""Compare reader additions with independent ECMA-335 decoding in .NET."""
import json
from pathlib import Path
import subprocess
from artifacts import executable

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


def check_reader(source, oracle_path, *, capture_output=False, facts_path=None):
    subprocess.run(["dotnet", "run", "--project", ROOT / "tests/metadata/MetadataOracle",
                    "-c", "Debug", "--", source, oracle_path],
                   check=True, capture_output=capture_output)
    facts = subprocess.run([executable("reader-facts"), source],
                           capture_output=True, check=True).stdout
    if facts_path is not None:
        facts_path.write_bytes(facts)
    actual = json.loads(facts)
    counts = compare(actual, json.loads(oracle_path.read_text()))
    return actual, counts


def main():
    out = ROOT / "tmp/verify"
    out.mkdir(parents=True, exist_ok=True)
    _, counts = check_reader(ROOT / "inputs/Windows.Win32.winmd", out / "oracle.json",
                             facts_path=out / "reader-facts.json")
    result = {"passed": True, "compared": counts, "scope": "Named type tokens in field/method signatures; modifiers and TypeSpec expansion are not a lossless AST guarantee"}
    (out / "reader-gate.json").write_text(json.dumps(result, indent=2))
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
