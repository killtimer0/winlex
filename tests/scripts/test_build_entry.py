"""Exercise the public build entry with real tools and controlled Cargo messages."""
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[2]
ARCH = os.environ.get("WINLEX_TEST_ARCH", "x64")


class BuildEntryContracts(unittest.TestCase):
    def run_entry(self, cargo_body, extra=""):
        script = f"""
$ErrorActionPreference = 'Stop'
. '{ROOT.as_posix()}/scripts/build-tools.ps1'
$policy = (Get-Content '{ROOT.as_posix()}/inputs/targets.json' -Raw | ConvertFrom-Json).{ARCH}
$tools = Initialize-WinlexToolchain {ARCH} $policy
function cargo {{ {cargo_body} }}
& '{ROOT.as_posix()}/scripts/build.ps1' -Architecture {ARCH} @tools {extra}
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

    def test_real_cargo_compile_error_is_rendered(self):
        with tempfile.TemporaryDirectory(dir=ROOT / "tmp") as folder:
            directory = Path(folder)
            (directory / "Cargo.toml").write_text(
                '[package]\nname="diagnostic-probe"\nversion="0.0.0"\nedition="2024"\n[workspace]\n',
                encoding="utf-8")
            (directory / "src").mkdir()
            (directory / "src/main.rs").write_text(
                '#[deprecated(note="contract warning")]\nfn old() {}\n'
                'fn main() { old(); compile_error!("contract compile failure"); }\n', encoding="utf-8")
            body = (f"& (Get-Command cargo.exe).Source build --manifest-path '{(directory / 'Cargo.toml').as_posix()}' "
                    "--message-format=json; $global:LASTEXITCODE=$LASTEXITCODE")
            result = self.run_entry(body)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("contract warning", result.stderr)
            self.assertIn("contract compile failure", result.stderr)
            self.assertIn("Build failed", result.stderr)

    def test_modified_dependency_is_rejected_before_cargo(self):
        with tempfile.TemporaryDirectory(prefix="输入 [contract] ", dir=ROOT / "tmp") as folder:
            import shutil
            policy = json.loads((ROOT / "inputs/targets.json").read_text())[ARCH]
            ltl = json.loads((ROOT / "inputs/vc-ltl-source.json").read_text())
            sdk = Path(folder) / "sdk"
            directory = sdk / "TargetPlatform" / policy["vc_ltl_platform"]
            directory.mkdir(parents=True)
            source = ROOT / f"tmp/deps/vc-ltl-{ltl['version']}/sdk/TargetPlatform" / policy["vc_ltl_platform"]
            for library in source.glob("*.lib"):
                shutil.copyfile(library, directory / library.name)
            (directory / "libucrt.lib").write_bytes(b"not the pinned library")
            result = self.run_entry("throw 'Cargo must not execute'", f"-VcLtlRoot '{sdk.as_posix()}'")
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("SHA256 mismatch", result.stderr)
            self.assertNotIn("Cargo must not execute", result.stderr)

    def test_utf8_cargo_paths_survive_non_utf8_console(self):
        with tempfile.TemporaryDirectory(prefix="中文 [cargo] ", dir=ROOT / "tmp") as folder:
            directory = Path(folder)
            messages = []
            for name in ("winlex", "winlex-gui"):
                path = directory / (name + ".exe")
                path.write_bytes(b"artifact fixture")
                messages.append({"reason": "compiler-artifact", "package_id": "fixture",
                                 "target": {"name": name, "kind": ["bin"]}, "executable": str(path)})
            output = ("\n".join(json.dumps(message, ensure_ascii=False) for message in messages) + "\n").encode("utf-8")
            body = f'& python -c "import sys;sys.stdout.buffer.write(bytes({list(output)}))"; $global:LASTEXITCODE=0'
            script = f"""
$ErrorActionPreference = 'Stop'
. '{ROOT.as_posix()}/scripts/build-tools.ps1'
$policy = (Get-Content '{ROOT.as_posix()}/inputs/targets.json' -Raw | ConvertFrom-Json).{ARCH}
$tools = Initialize-WinlexToolchain {ARCH} $policy
[Console]::OutputEncoding = [Text.Encoding]::GetEncoding(936)
function cargo {{ {body} }}
& '{ROOT.as_posix()}/scripts/build.ps1' -Architecture {ARCH} @tools
if ([Console]::OutputEncoding.CodePage -ne 936) {{ throw 'Console encoding was not restored' }}
"""
            result = subprocess.run(["pwsh", "-NoProfile", "-Command", script], cwd=ROOT,
                                    capture_output=True)
            self.assertEqual(result.returncode, 0, (result.stdout + result.stderr).decode("utf-8", errors="replace"))
            for name in ("winlex", "winlex-gui"):
                record = json.loads((directory / f"{name}.exe.build.json").read_text(encoding="utf-8-sig"))
                self.assertEqual(Path(record["executable"]), directory / f"{name}.exe")


if __name__ == "__main__":
    unittest.main()
