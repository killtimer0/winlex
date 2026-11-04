"""Exercise the public build entry with real tools and controlled Cargo messages."""
import json
from pathlib import Path
import subprocess
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[2]


class BuildEntryContracts(unittest.TestCase):
    def run_entry(self, cargo_body, extra=""):
        script = f"""
$ErrorActionPreference = 'Stop'
. '{ROOT.as_posix()}/scripts/build-tools.ps1'
$policy = (Get-Content '{ROOT.as_posix()}/inputs/targets.json' -Raw | ConvertFrom-Json).x64
$tools = Initialize-WinlexToolchain x64 $policy
function cargo {{ {cargo_body} }}
& '{ROOT.as_posix()}/scripts/build.ps1' -Architecture x64 @tools {extra}
"""
        return subprocess.run(["pwsh", "-NoProfile", "-Command", script], cwd=ROOT,
                              text=True, capture_output=True, encoding="utf-8", errors="replace")

    def test_cargo_diagnostics_survive_failed_build(self):
        messages = [json.dumps({"reason": "compiler-message", "message": {"rendered": text}})
                    for text in ("warning: contract warning\n", "error: contract compile failure\n")]
        body = "; ".join("'" + message + "'" for message in messages) + "; $global:LASTEXITCODE = 1"
        result = self.run_entry(body)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("warning: contract warning", result.stderr)
        self.assertIn("error: contract compile failure", result.stderr)
        self.assertIn("Build failed", result.stderr)

    def test_modified_dependency_is_rejected_before_cargo(self):
        with tempfile.TemporaryDirectory(prefix="输入 [contract] ", dir=ROOT / "tmp") as folder:
            obj = Path(folder) / "bad.obj"
            obj.write_bytes(b"not the pinned object")
            result = self.run_entry("throw 'Cargo must not execute'", f"-YyThunksObject '{obj.as_posix()}'")
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("SHA256 mismatch", result.stderr)
            self.assertNotIn("Cargo must not execute", result.stderr)


if __name__ == "__main__":
    unittest.main()
