import json
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parents[1]
EXE = ROOT / "target/release/examples/host-cli.exe"


def layout(source, name):
    return subprocess.run([EXE, "--winmd", source, "--json", "--arch", "x64", "layout", name], capture_output=True, text=True, encoding="utf-8")


def main():
    out = ROOT / "tmp/verify/reader-fixtures"
    out.mkdir(parents=True, exist_ok=True)
    subprocess.run(["dotnet", "build", str(ROOT / "verify/ReaderFixtures"), "-c", "Release", "-o", str(out)], check=True)
    source = out / "ReaderFixtures.dll"
    subprocess.run(["dotnet", "run", "--project", str(ROOT / "verify/MetadataOracle"), "-c", "Release", "--", str(source), str(out / "oracle.json")], check=True)
    facts = subprocess.run([ROOT / "target/release/examples/reader-facts.exe", source], capture_output=True, check=True)
    facts = json.loads(facts.stdout)
    oracle = json.loads((out / "oracle.json").read_text())
    for key in ("layouts", "scopes", "signature_type_tokens"):
        assert facts[key] == oracle[key], key
    assert any(r["offset"] == 8 for r in facts["layouts"])
    for name, reason in (("Nonzero", "nonzero/missing explicit offset"), ("SizedOverlay", "declared size"), ("External", "external ResolutionScope")):
        result = layout(source, name)
        assert result.returncode and reason in json.loads(result.stdout)["error"]["message"], result.stdout
    result = layout(source, "Holder")
    result.check_returncode()
    holder = json.loads(result.stdout)["result"]
    assert holder["size_bits"] == 128 and [f["offset_bits"] for f in holder["fields"]] == [0, 64]
    assert [f["fields"][0]["size_bits"] for f in holder["fields"]] == [32, 64]
    result = layout(source, "Overlay")
    result.check_returncode()
    overlay = json.loads(result.stdout)["result"]
    assert overlay["size_bits"] == 64 and [f["offset_bits"] for f in overlay["fields"]] == [0, 0]
    summary = {"passed": True, "checks": ["independent raw offsets/scopes/signature tokens", "nonzero offset retained but layout rejected", "declared size rejected", "external assembly scope rejected instead of name binding", "same-name nested TypeDef targets kept distinct", "all-zero overlay layout"]}
    (out / "report.json").write_text(json.dumps(summary, indent=2))
    print(json.dumps(summary, indent=2))


if __name__ == "__main__":
    main()
