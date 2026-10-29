import json
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parents[2]
EXE = ROOT / "target/release/examples/host-cli.exe"


def layout(source, name):
    return subprocess.run([EXE, source, "--json", "--arch", "x64", "layout", name], capture_output=True, text=True, encoding="utf-8")


def main():
    out = ROOT / "tmp/verify/reader-fixtures"
    out.mkdir(parents=True, exist_ok=True)
    subprocess.run(["dotnet", "build", str(ROOT / "tests/metadata/ReaderFixtures"), "-c", "Release", "-o", str(out)], check=True)
    source = out / "ReaderFixtures.dll"
    subprocess.run(["dotnet", "run", "--project", str(ROOT / "tests/metadata/MetadataOracle"), "-c", "Release", "--", str(source), str(out / "oracle.json")], check=True)
    facts = subprocess.run([ROOT / "target/release/examples/reader-facts.exe", source], capture_output=True, check=True)
    facts = json.loads(facts.stdout)
    oracle = json.loads((out / "oracle.json").read_text())
    for key in ("assembly", "references", "layouts", "scopes", "signature_type_tokens", "properties", "events"):
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
    def query(command, name):
        p = subprocess.run([EXE, source, "--json", "--arch", "x64", command, name], capture_output=True, check=True)
        return json.loads(p.stdout)["result"]
    box = query("show", "Fixtures.IBox`1")
    assert box["generic_parameters"][0]["name"] == "T"
    prop = box["properties"][0]
    assert prop["type"] == {"kind": "generic", "scope": "type", "name": "T", "index": 0}
    assert sorted(a["semantics"] for a in prop["accessors"]) == [1, 2]
    entity = prop["entity"]["id"]
    assert query("show", f'{entity["source_id"]}::0x{entity["token"]:08x}')["entity"] == prop["entity"]
    event = box["events"][0]
    assert event["type"]["arguments"][0]["name"] == "T"
    assert sorted(a["semantics"] for a in event["accessors"]) == [8, 16]
    convert = next(m for m in box["methods"] if m["entity"]["name"] == "Convert")
    assert convert["return_type"] == {"kind": "generic", "scope": "method", "name": "U", "index": 0}
    assert [p["type"]["scope"] for p in convert["parameters"]] == ["type", "method"]
    arrays = next(m for m in box["methods"] if m["entity"]["name"] == "Arrays")
    assert arrays["return_type"]["arguments"][0]["element"]["name"] == "T"
    assert query("show", "Fixtures.Box`1")["fields"][0]["type"]["name"] == "T"
    query("references", "Fixtures.IBox`1")
    one = query("references", "Fixtures.Box`1")["page"]["matches"]
    two = query("references", "Fixtures.Box`2")["page"]["matches"]
    assert [m["name"] for m in one] == ["One"]
    assert [m["name"] for m in two] == ["Two"]
    assert layout(source, "Fixtures.Box`1").returncode == 3
    summary = {"passed": True, "checks": ["independent raw offsets/scopes/signature tokens/properties/events", "nonzero offset retained but layout rejected", "declared size rejected", "external assembly scope rejected instead of name binding", "same-name nested TypeDef targets kept distinct", "all-zero overlay layout", "type and method generic parameters", "generic array arguments", "property/event accessor navigation", "open generic ABI rejection"]}
    (out / "report.json").write_text(json.dumps(summary, indent=2))
    print(json.dumps(summary, indent=2))


if __name__ == "__main__":
    main()
