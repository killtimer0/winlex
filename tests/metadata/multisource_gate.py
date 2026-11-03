"""Build independent C# assemblies and exercise source-qualified queries."""
import json
from pathlib import Path
import subprocess
from reader_gate import check_reader
from artifacts import executable

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / "tmp/verify/multisource"


def build(folder, name, version, code, dependency=None, public_sign=False):
    path = OUT / folder
    path.mkdir(parents=True, exist_ok=True)
    reference = f'<ItemGroup><Reference Include="Base"><HintPath>{dependency}</HintPath></Reference></ItemGroup>' if dependency else ''
    signing = ''
    if public_sign:
        # Public-only fixture key; no cryptographic signature is produced.
        (path / 'key.snk').write_bytes(bytes.fromhex('00240000048000009400000006020000002400005253413100040000010001008D56C76F9E8649383049F383C44BE0EC204181822A6C31CF5EB7EF486944D032188EA1D3920763712CCB12D75FB77E9811149E6148E5D32FBAAB37611C1878DDC19E20EF135D0CB2CFF2BFEC3D115810C3D9069638FE4BE215DBF795861920E5AB6F7DB2E2CEEF136AC23D5DD2BF031700AEC232F6C6B1C785B4305C123B37AB'))
        signing = '<SignAssembly>true</SignAssembly><PublicSign>true</PublicSign><AssemblyOriginatorKeyFile>key.snk</AssemblyOriginatorKeyFile>'
    (path / "fixture.csproj").write_text(f'<Project Sdk="Microsoft.NET.Sdk"><PropertyGroup><TargetFramework>net8.0</TargetFramework><AssemblyName>{name}</AssemblyName><AssemblyVersion>{version}</AssemblyVersion>{signing}</PropertyGroup>{reference}</Project>')
    (path / "Types.cs").write_text(code)
    subprocess.run(["dotnet", "build", path / "fixture.csproj", "-c", "Debug", "-o", path / "out"], check=True, capture_output=True)
    return path / "out" / f"{name}.dll"


def query(files, command, name=None, *options, error=None):
    args = [executable("host-cli"), "--json", "--arch", "x64"]
    for file in files:
        args.append(file)
    args.extend(options)
    args.append(command)
    if name is not None:
        args.append(name)
    p = subprocess.run(args, capture_output=True)
    data = json.loads(p.stdout)
    if error:
        assert p.returncode and error in data["error"]["message"], data
        return data
    assert p.returncode == 0, data
    return data


def main():
    code = 'public struct Global { public int X; } namespace Shared { public struct Item { public long X; } public class Outer { public struct Child { public int X; } } public interface IBase { void First(); } }'
    base = build("base", "Base", "1.2.300.400", code)
    other = build("other", "Other", "1.0.0.0", code)
    wrong = build("wrong", "Base", "1.2.300.401", code)
    duplicate = build("duplicate", "Base", "1.2.300.400", code + ' public struct Extra { public int X; }')
    consumer = build("consumer", "Consumer", "1.0.0.0", 'namespace Client { public struct Holder { public Shared.Item A; public Shared.Outer.Child B; public Global C; } public interface IDerived : Shared.IBase { void Second(); } }', base)
    files = [consumer, base, other]
    source = 1
    layout = query(files, "layout", "Holder")["result"]
    assert layout["size_bits"] == 128
    assert [f["offset_bits"] for f in layout["fields"]] == [0, 64, 96]
    assert all(f["fields"][0]["id"]["source_id"] == source for f in layout["fields"])
    slots = query(files, "vtable", "IDerived")["result"]["slots"]
    assert [s["method"] for s in slots] == ["First", "Second"]
    assert slots[0]["id"]["source_id"] == source != slots[1]["id"]["source_id"]
    matches = query(files, "search", "Item", "--name")["result"]["matches"]
    assert [m["kind"] for m in matches] == ["type_reference", "struct", "struct"]
    definitions = matches[1:]
    assert definitions[0]["id"]["token"] == definitions[1]["id"]["token"] and definitions[0]["id"] != definitions[1]["id"]
    query(files, "show", "Item", error="found 2")
    query(files, "show", f'0x{definitions[0]["id"]["token"]:08x}', error="requires SOURCE")
    assert query(files, "show", "Base::Shared.Item")["result"]["entity"]["id"]["source_id"] == source
    page = query(files, "search", "Item", "--name", "--source", "Base")["result"]
    assert len(page["matches"]) == 1
    reversed_matches = query(files[::-1], "search", "Item", "--name")["result"]["matches"]
    assert [m["id"]["source_id"] for m in reversed_matches] == [0, 1, 2]
    assert [m["kind"] for m in reversed_matches] == ["struct", "struct", "type_reference"]
    assert query(files, "search", "Item", "--name", "--limit", "1", "--offset", "1")["result"]["matches"] == matches[1:2]
    assert len(query([base, base], "summary")["sources"]) == 2
    uses = query(files, "references", "Base::Shared.Item")["result"]["page"]["matches"]
    assert len(uses) == 1 and uses[0]["name"] == "A"
    for deps in ([consumer], [consumer, wrong], [consumer, other]):
        query(deps, "layout", "Holder", error="0 matching sources")
    query([consumer, base, duplicate], "layout", "Holder", error="2 matching sources")
    query([base, duplicate], "show", "Base::Shared.Item", error="matches 2 files")
    signed = build("signed", "Base", "1.2.300.400", code, public_sign=True)
    signed_user = build("signed-user", "SignedUser", "1.0.0.0", 'public struct SignedHolder { public Shared.Item A; }', signed)
    assert query([signed_user, signed, base], "layout", "SignedHolder")["result"]["size_bits"] == 64
    query([signed_user, base], "layout", "SignedHolder", error="0 matching sources")
    for file in (base, consumer, signed, signed_user):
        check_reader(file, file.parent / "oracle.json", capture_output=True)
    print("Multi-source layouts, nested/global references, COM inheritance, identity conflicts and independent reader checks passed")


if __name__ == "__main__":
    main()
