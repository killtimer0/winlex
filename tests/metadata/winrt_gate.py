"""Validate WinRT queries against SDK metadata, headers and the .NET reader."""
import argparse
import json
from pathlib import Path
import re
import subprocess
from artifacts import executable

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / "tmp/verify/winrt"


def query(files, command, name, error=None):
    args = [executable("host-cli"), "--json", "--arch", "x64", "--limit", "1000"]
    for file in files:
        args.append(file)
    p = subprocess.run(args + [command, name], capture_output=True)
    data = json.loads(p.stdout)
    if error:
        assert p.returncode == 3 and error in data["error"]["message"], data
        return data
    assert p.returncode == 0, data
    return data["result"]


def references(value):
    if isinstance(value, dict):
        if value.get("reference_id"):
            yield value
        for child in value.values():
            yield from references(child)
    elif isinstance(value, list):
        for child in value:
            yield from references(child)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--sdk", type=Path, required=True, help="Windows Kits/10 directory")
    parser.add_argument("--version", default="10.0.26100.0")
    parser.add_argument("--apps", type=Path, help="directory containing extracted App SDK packages")
    args = parser.parse_args()
    OUT.mkdir(parents=True, exist_ok=True)
    union = args.sdk / "UnionMetadata" / args.version / "Windows.winmd"
    contracts = list((args.sdk / "References" / args.version).rglob("*.winmd"))
    assert contracts
    specialization_cases = [
        ("Windows.Foundation.Collections.IMap<string,object>", "windows.foundation.h", "Windows.Foundation.Collections.IMap`2<String, Object>"),
        ("Windows.Foundation.Collections.IIterator<Windows.Foundation.Collections.IKeyValuePair<string,object>>", "windows.foundation.h", "Windows.Foundation.Collections.IIterator`1<Windows.Foundation.Collections.IKeyValuePair`2<String, Object>>"),
        ("Windows.Foundation.IReference<Windows.Foundation.TimeSpan>", "windows.applicationmodel.appointments.h", "Windows.Foundation.IReference`1<Windows.Foundation.TimeSpan>"),
        ("Windows.Foundation.Collections.IIterable<Windows.Storage.StorageFile>", "windows.applicationmodel.activation.h", "Windows.Foundation.Collections.IIterable`1<Windows.Storage.StorageFile>"),
        ("Windows.Foundation.Collections.IVector<Windows.Devices.Enumeration.DeviceClass>", "windows.devices.enumeration.h", "Windows.Foundation.Collections.IVector`1<Windows.Devices.Enumeration.DeviceClass>"),
        ("Windows.Foundation.TypedEventHandler<Windows.Storage.ApplicationData,object>", "windows.storage.h", "Windows.Foundation.TypedEventHandler`2<Windows.Storage.ApplicationData, Object>"),
        ("Windows.Foundation.IAsyncOperation<guid>", "windows.applicationmodel.calls.h", "Windows.Foundation.IAsyncOperation`1<Guid>"),
    ]
    sdk_iids = {}
    for expression, filename, runtime_name in specialization_cases:
        header = (args.sdk / "Include" / args.version / "winrt" / filename).read_text(errors="replace")
        found = [iid for iid, body in re.findall(r'struct __declspec\(uuid\("([^"]+)"\)\)\s*(.*?)\n};', header, re.S)
                 if f'return L"{runtime_name}";' in body]
        assert len(found) == 1, (expression, found)
        sdk_iids[expression] = found[0]
    for files in ([union], contracts):
        vector = query(files, "show", "Windows.Foundation.Collections.IVector`1")
        assert vector["generic_parameters"][0]["name"] == "T"
        get_at = next(m for m in vector["methods"] if m["entity"]["name"] == "GetAt")
        assert get_at["return_type"]["scope"] == "type"
        assert get_at["return_type"]["index"] == 0
        query(files, "vtable", "Windows.Foundation.Collections.IVector`1", error="open generic")
        expected_iids = {
            "IVector<int32>": "b939af5b-b45d-5489-9149-61442c1905fe",
            "IIterable<int32>": "81a643fb-f51c-5565-83c4-f96425777b66",
            "IVectorView<int32>": "8d720cdf-3934-5d3f-9a55-40e8063b086a",
        }
        for expression, iid in expected_iids.items():
            identity = query(files, "iid", "Windows.Foundation.Collections." + expression)
            assert identity["iid"] == iid, identity
        for expression, iid in sdk_iids.items():
            identity = query(files, "iid", expression)
            assert identity["iid"] == iid, (expression, identity, iid)
        bound = query(files, "show", "Windows.Foundation.Collections.IVector<int32>")
        get_at = next(m for m in bound["methods"] if m["entity"]["name"] == "GetAt")
        assert get_at["return_type"]["name"] == "int32"
        table = query(files, "vtable", "Windows.Foundation.Collections.IVector<int32>")
        assert table["slots"][6]["method"] == "GetAt"
        assert table["slots"][6]["byte_offset"] == 48
        methods = {table["slots"][m["slot"]]["method"]: m for m in table["instance"]["methods"]}
        assert methods["GetAt"]["abi"]["return_type"] == "HRESULT"
        assert methods["GetAt"]["abi"]["parameters"][-1]["ty"] == "int32*"
        assert methods["IndexOf"]["abi"]["parameters"][-1]["ty"] == "boolean*"
        assert "unsupported" in methods["GetMany"] and "abi" not in methods["GetMany"]
        slots = query(files, "vtable", "Windows.Foundation.IAsyncInfo")["slots"]
        assert [s["method"] for s in slots[:6]] == [
            "QueryInterface", "AddRef", "Release", "GetIids", "GetRuntimeClassName", "GetTrustLevel",
        ]
        assert slots[6]["method"] == "get_Id" and slots[6]["byte_offset"] == 48
        delegate = query(files, "vtable", "Windows.Foundation.AsyncActionCompletedHandler")["slots"]
        assert len(delegate) == 4 and delegate[3]["method"] == "Invoke"
        # Required IAsyncInfo must not be appended to IAsyncAction's vtable.
        action = query(files, "vtable", "Windows.Foundation.IAsyncAction")["slots"]
        assert [s["method"] for s in action[6:]] == ["put_Completed", "get_Completed", "GetResults"]
        assert query(files, "layout", "Windows.Foundation.Point")["size_bits"] == 64
        assert query(files, "references", "Windows.Foundation.Point")["page"]["total"] > 0
        uses = query(files, "references", "Windows.Foundation.Collections.IVector`1")
        assert uses["page"]["total"] > 0
        storage = query(files, "show", "Windows.Storage.StorageFile")
        assert storage["properties"] and storage["base_type"]
        assert sum(i["default"] for i in storage["interface_implementations"]) == 1
    header = (args.sdk / "Include" / args.version / "winrt/AsyncInfo.h").read_text(errors="replace")
    assert re.search(r"IAsyncInfo\s*:\s*public IInspectable", header)
    block = header.split("typedef struct IAsyncInfoVtbl", 1)[1].split("} IAsyncInfoVtbl", 1)[0]
    assert re.findall(r"STDMETHODCALLTYPE\s*\*(\w+)", block) == [s["method"] for s in slots]
    # These specializations and UUIDs are emitted by the SDK's MIDL compiler.
    sms = (args.sdk / "Include" / args.version / "winrt/windows.devices.sms.h").read_text(errors="replace").lower()
    for iid in expected_iids.values():
        assert iid in sms, iid
    apps = sorted(set(args.apps.glob("**/metadata/*.winmd"))) if args.apps else []
    subprocess.run([executable("query-audit"), union, *apps], check=True)
    if apps:
        for deps in ([union], contracts):
            files = apps + deps
            app = query(files, "show", "Microsoft.Windows.AppLifecycle.AppInstance")
            refs = {r["reference_id"]: r for r in references(app) if r["namespace"].startswith("Windows.")}
            assert refs
            for ref in refs:
                result = query(files, "show", ref)
                assert result["resolution"].get("target_id"), result
            assert query(files, "layout", "Microsoft.UI.Xaml.Thickness")["size_bits"] == 256
            query(files, "show", "Microsoft.UI.Xaml.Controls.Button")
            query(files, "references", "Windows.Foundation.Point")
        ref = next(iter(refs))
        missing = query(apps, "show", ref)
        assert "0 matching definitions" in missing["resolution"]["unresolved"]
        overlap = query(apps + contracts + [union], "show", ref)
        assert "2 matching definitions" in overlap["resolution"]["unresolved"]
    checks = [
        "generic declarations",
        "closed generic signatures and SDK specialization IIDs",
        "generic WinRT slots, scalar return lowering and explicit array ABI rejection",
        "properties/events/accessors",
        "default interfaces",
        "interface and delegate ABI slots",
        "reverse generic uses",
    ]
    if apps:
        checks.extend(["WinRT namespace resolution", "missing/overlapping sources"])
    report = {"passed": True, "sdk": args.version, "contracts": len(contracts), "app_files": len(apps),
              "checks": checks, "skipped": [] if apps else ["App SDK checks: no metadata inputs"]}
    (OUT / "report.json").write_text(json.dumps(report, indent=2))
    print(json.dumps(report, indent=2))


if __name__ == "__main__":
    main()
