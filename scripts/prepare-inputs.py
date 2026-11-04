"""Prepare the pinned YY-Thunks and VC-LTL5 build dependencies."""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import urllib.request
import zipfile

ROOT = Path(__file__).resolve().parents[1]
DEPS = ROOT / "tmp/deps"


def manifest(name):
    return json.loads((ROOT / "inputs" / name).read_text(encoding="utf-8-sig"))


def check(path, expected):
    actual = hashlib.sha256(path.read_bytes()).hexdigest()
    if actual != expected:
        raise ValueError(f"SHA256 mismatch: {path}: {actual} != {expected}")


def download(url, destination, sha256=None):
    destination.parent.mkdir(parents=True, exist_ok=True)
    if not destination.exists():
        partial = destination.with_suffix(destination.suffix + ".partial")
        print(f"Downloading {url}", flush=True)
        with urllib.request.urlopen(url, timeout=120) as source, partial.open("wb") as out:
            shutil.copyfileobj(source, out)
        if sha256:
            check(partial, sha256)
        partial.replace(destination)
    elif sha256:
        check(destination, sha256)
    return destination


def install(path, data, sha256):
    path.parent.mkdir(parents=True, exist_ok=True)
    partial = path.with_suffix(path.suffix + ".partial")
    try:
        partial.write_bytes(data)
        check(partial, sha256)
        partial.replace(path)
    finally:
        partial.unlink(missing_ok=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--build-deps", action="store_true", help="compatibility alias (dependencies are prepared by default)")
    parser.parse_args()
    thunks = manifest("yy-thunks-source.json")
    directory = DEPS / f"yy-thunks-{thunks['version']}"
    archive = download(thunks["url"], directory / "objects.zip", thunks["archive_sha256"])
    with zipfile.ZipFile(archive) as package:
        for arch, sha in thunks["objects"].items():
            filename = f"{arch}/YY_Thunks_for_WinXP.obj"
            names = [name for name in package.namelist() if name.endswith(filename)]
            if len(names) != 1:
                raise ValueError(f"Expected one object for {arch}")
            path = directory / filename
            install(path, package.read(names[0]), sha)
    ltl = manifest("vc-ltl-source.json")
    directory = DEPS / f"vc-ltl-{ltl['version']}"
    archive = download(ltl["url"], directory / "sdk.7z", ltl["archive_sha256"])
    sdk = directory / "sdk"
    sevenzip = shutil.which("7z") or shutil.which("7zz")
    if not sevenzip:
        raise SystemExit("Add 7z or 7zz to PATH to extract VC-LTL5")
    subprocess.run([sevenzip, "x", str(archive), f"-o{sdk}", "-y"], check=True)
    for platform, libraries in ltl["libraries"].items():
        for library in libraries:
            check(sdk / "TargetPlatform" / platform / library["name"], library["sha256"])
    print(f"Verified build dependencies in {DEPS}")


if __name__ == "__main__":
    main()
