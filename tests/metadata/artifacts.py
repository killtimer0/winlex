"""Cargo-reported gate executables, shared by the runner and standalone gates."""
import json
import os
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[2]
DEFAULT_MANIFEST = ROOT / "tmp/verify/artifacts.json"
OVERRIDES = {
    "host-cli": "WINLEX_GATE_CLI",
    "reader-facts": "WINLEX_GATE_READER_FACTS",
    "query-audit": "WINLEX_GATE_QUERY_AUDIT",
}


def select_executable(messages, name):
    paths = {
        message["executable"]
        for message in messages
        if message.get("reason") == "compiler-artifact"
        and message.get("target", {}).get("name") == name
        and "example" in message.get("target", {}).get("kind", [])
        and not message.get("profile", {}).get("test", False)
        and message.get("executable")
    }
    if len(paths) != 1:
        raise RuntimeError(f"Cargo must report exactly one executable for example {name}: {sorted(paths)}")
    return Path(paths.pop()).absolute()


def build_example(package, name, options):
    command = ["cargo", "build", "-p", package, "--example", name, "--locked",
               "--message-format=json-render-diagnostics", *options]
    result = subprocess.run(command, cwd=ROOT, stdout=subprocess.PIPE, text=True, encoding="utf-8")
    messages = [json.loads(line) for line in result.stdout.splitlines() if line.strip()]
    for message in messages:
        rendered = message.get("message", {}).get("rendered")
        if rendered:
            print(rendered, end="", file=sys.stderr)
    result.check_returncode()
    return select_executable(messages, name)


def manifest_path():
    return Path(os.environ.get("WINLEX_GATE_ARTIFACTS", DEFAULT_MANIFEST)).absolute()


def write_manifest(executables, path):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps({"version": 1, "executables": {
        name: str(Path(value).absolute()) for name, value in executables.items()
    }}, indent=2) + "\n", encoding="utf-8")


def executable(name):
    override = os.environ.get(OVERRIDES[name])
    if override:
        path = Path(override).absolute()
    else:
        manifest = manifest_path()
        try:
            data = json.loads(manifest.read_text(encoding="utf-8"))
            if not isinstance(data, dict) or data.get("version") != 1:
                raise ValueError("unsupported manifest version")
            path = Path(data["executables"][name])
            if not path.is_absolute():
                path = manifest.parent / path
        except (OSError, ValueError, KeyError, TypeError) as error:
            raise RuntimeError(f"Cannot select {name} from {manifest}; run tests/metadata/run.py "
                               f"or set {OVERRIDES[name]}: {error}") from error
    if not path.is_file():
        raise RuntimeError(f"Gate executable does not exist: {path}")
    return path
