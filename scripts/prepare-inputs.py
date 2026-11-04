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


def policy(architecture):
    return manifest("targets.json")[architecture]


def verify_inputs(architecture, sdk, object_path=None):
    target = policy(architecture)
    libraries = manifest("vc-ltl-source.json")["libraries"][target["vc_ltl_platform"]]
    directory = sdk / "TargetPlatform" / target["vc_ltl_platform"]
    expected = {library["name"] for library in libraries}
    actual = {path.name for path in directory.glob("*.lib")}
    if actual != expected:
        raise ValueError(f"VC-LTL library set mismatch: {directory}")
    for library in libraries:
        check(directory / library["name"], library["sha256"])
    result = {"vc_ltl_libraries": libraries}
    if target["thunks_arch"]:
        if object_path is None:
            raise ValueError("Specify the YY-Thunks object for this target")
        digest = manifest("yy-thunks-source.json")["objects"][target["thunks_arch"]]
        check(object_path, digest)
        result["thunks_sha256"] = digest
    return result


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
    parser.add_argument("--architecture", choices=("x86", "x64", "arm64"))
    parser.add_argument("--check", action="store_true", help="verify local inputs without downloading; emit JSON")
    parser.add_argument("--vc-ltl-root", type=Path)
    parser.add_argument("--thunks-object", type=Path)
    args = parser.parse_args()
    if args.check:
        if not args.architecture or not args.vc_ltl_root:
            parser.error("--check requires --architecture and --vc-ltl-root")
        print(json.dumps(verify_inputs(args.architecture, args.vc_ltl_root, args.thunks_object)))
        return
    targets = manifest("targets.json")
    selected = [targets[args.architecture]] if args.architecture else list(targets.values())
    thunk_arches = {target["thunks_arch"] for target in selected} - {None}
    if thunk_arches:
        thunks = manifest("yy-thunks-source.json")
        directory = DEPS / f"yy-thunks-{thunks['version']}"
        archive = download(thunks["url"], directory / "objects.zip", thunks["archive_sha256"])
        with zipfile.ZipFile(archive) as package:
            for arch in sorted(thunk_arches):
                filename = f"{arch}/YY_Thunks_for_WinXP.obj"
                names = [name for name in package.namelist() if name.endswith(filename)]
                if len(names) != 1:
                    raise ValueError(f"Expected one object for {arch}")
                install(directory / filename, package.read(names[0]), thunks["objects"][arch])
    ltl = manifest("vc-ltl-source.json")
    directory = DEPS / f"vc-ltl-{ltl['version']}"
    archive = download(ltl["url"], directory / "sdk.7z", ltl["archive_sha256"])
    sdk = directory / "sdk"
    sevenzip = shutil.which("7z") or shutil.which("7zz")
    if not sevenzip:
        raise SystemExit("Add 7z or 7zz to PATH to extract VC-LTL5")
    subprocess.run([sevenzip, "x", str(archive), f"-o{sdk}", "-y"], check=True)
    for platform in {target["vc_ltl_platform"] for target in selected}:
        libraries = ltl["libraries"][platform]
        for library in libraries:
            check(sdk / "TargetPlatform" / platform / library["name"], library["sha256"])
    print(f"Verified build dependencies in {DEPS}")


if __name__ == "__main__":
    main()
