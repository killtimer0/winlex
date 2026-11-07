"""Prepare the pinned Win32 metadata input used by the Rust and metadata tests."""
import argparse
import hashlib
import io
import json
from pathlib import Path
import urllib.request
import zipfile

ROOT = Path(__file__).resolve().parents[2]


def prepare(source, destination):
    expected = source["sha256"]
    if destination.is_file() and hashlib.sha256(destination.read_bytes()).hexdigest() == expected:
        return
    package = source["package"].lower()
    version = source["version"].lower()
    url = f"https://api.nuget.org/v3-flatcontainer/{package}/{version}/{package}.{version}.nupkg"
    print(f"Downloading {url}", flush=True)
    with urllib.request.urlopen(url, timeout=120) as response:
        archive = response.read()
    with zipfile.ZipFile(io.BytesIO(archive)) as package:
        candidates = [name for name in package.namelist() if Path(name).name == source["file"]]
        if len(candidates) != 1:
            raise ValueError(f"Expected exactly one {source['file']} in the NuGet package: {candidates}")
        data = package.read(candidates[0])
    actual = hashlib.sha256(data).hexdigest()
    if actual != expected:
        raise ValueError(f"SHA256 mismatch for {source['file']}: {actual} != {expected}")
    destination.parent.mkdir(parents=True, exist_ok=True)
    partial = destination.with_suffix(destination.suffix + ".partial")
    try:
        partial.write_bytes(data)
        partial.replace(destination)
    finally:
        partial.unlink(missing_ok=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, help="override the default inputs/ destination")
    parser.add_argument("--foundation-reference", action="store_true", help="prepare the official GUID regression reference DLL")
    args = parser.parse_args()
    manifest = "foundation-reference.json" if args.foundation_reference else "source.json"
    source = json.loads(Path(__file__).with_name(manifest).read_text(encoding="utf-8"))
    directory = ROOT / "tmp/verify/reference" if args.foundation_reference else ROOT / "inputs"
    destination = args.output if args.output is not None else directory / source["file"]
    prepare(source, destination)
    print(f"Verified {destination}: {source['sha256']}")


if __name__ == "__main__":
    main()
