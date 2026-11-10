"""Read the workspace package version and validate release tags."""
import argparse
from pathlib import Path
import tomllib


ROOT = Path(__file__).resolve().parents[1]


def package_version(root=ROOT):
    manifest = tomllib.loads((root / "Cargo.toml").read_text(encoding="utf-8"))
    try:
        return manifest["workspace"]["package"]["version"]
    except KeyError:
        raise ValueError("Cargo.toml has no workspace package version") from None


def check_tag(tag, root=ROOT):
    expected = "v" + package_version(root)
    # The release workflow marks tags with a suffix as prereleases.
    prerelease = tag.startswith(expected + "-") and len(tag) > len(expected) + 1
    if tag != expected and not prerelease:
        raise ValueError("Tag %s does not match package version %s" % (tag, expected))


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--tag", help="release tag to compare with the package version")
    args = parser.parse_args()
    if args.tag is None:
        print(package_version())
    else:
        check_tag(args.tag)
        print("PASS release tag", args.tag)


if __name__ == "__main__":
    main()
