"""Capture source and toolchain identities for build and packaging contracts."""
import hashlib
import json
import os
from pathlib import Path
import subprocess


ROOT = Path(__file__).resolve().parents[1]


def command(*args, cwd=None):
    return subprocess.check_output(args, cwd=cwd).decode("utf-8").strip()


def tree_hash(root, names):
    digest = hashlib.sha256()
    for name in sorted(set(names)):
        path = root / name
        data = (os.readlink(path).encode("utf-8") if path.is_symlink() else
                path.read_bytes() if path.is_file() else b"<missing>")
        digest.update(name.encode("utf-8") + b"\0")
        digest.update(hashlib.sha256(data).digest())
    return digest.hexdigest()


def snapshot(root=ROOT):
    names = subprocess.check_output(
        ["git", "ls-files", "--cached", "--others", "--exclude-standard", "-z"], cwd=root
    ).decode("utf-8").rstrip("\0").split("\0")
    source = {
        "commit": command("git", "rev-parse", "HEAD", cwd=root),
        "dirty": bool(command("git", "status", "--porcelain", "--untracked-files=all", cwd=root)),
        "sha256": tree_hash(root, names),
    }
    sysroot = Path(command("rustc", "--print", "sysroot"))
    docs = sysroot / "share/doc/rust"
    licenses = ["COPYRIGHT-library.html", *(
        path.relative_to(docs).as_posix() for path in (docs / "licenses").rglob("*") if path.is_file()
    )]
    license_hash = (tree_hash(docs, licenses)
                    if (docs / "COPYRIGHT-library.html").is_file() and len(licenses) > 1 else None)
    return {
        "source": source,
        "rustc": command("rustc", "-Vv"),
        "cargo": command("cargo", "--version"),
        "rust_licenses_sha256": license_hash,
    }


if __name__ == "__main__":
    print(json.dumps(snapshot()))
