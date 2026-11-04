"""Packaging contracts using explicitly selected or standard Debug build records."""
import hashlib
import json
import os
import re
import subprocess
import tempfile
import unittest
from pathlib import Path
from zipfile import ZipFile


ROOT = Path(__file__).resolve().parents[2]
TARGETS = {"x86": "i686-pc-windows-msvc", "x64": "x86_64-pc-windows-msvc"}
SCRIPT = ROOT / "scripts/package.ps1"


class PackageContracts(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        (ROOT / "tmp").mkdir(exist_ok=True)
        cls.temp = tempfile.TemporaryDirectory(prefix="package-contract-", dir=ROOT / "tmp")
        cls.directory = Path(cls.temp.name)

    @classmethod
    def tearDownClass(cls):
        cls.temp.cleanup()

    def records(self, arch):
        binary_dir = Path(os.environ.get(f"WINLEX_PACKAGE_{arch.upper()}_BUILD_DIR",
                                        ROOT / "target" / TARGETS[arch] / "debug"))
        records = [binary_dir / f"{name}.exe.build.json" for name in ("winlex", "winlex-gui")]
        if not all(path.is_file() for path in records):
            self.skipTest(f"Debug build records not available: {binary_dir}")
        return records

    def package(self, arch, records, output, configuration="Debug"):
        return subprocess.run([
            "pwsh", "-NoProfile", "-File", str(SCRIPT), "-Architecture", arch,
            "-Configuration", configuration, "-CliBuildRecord", str(records[0]),
            "-GuiBuildRecord", str(records[1]), "-OutputDir", str(output),
        ], text=True, capture_output=True, encoding="utf-8", errors="replace")

    def test_debug_packages(self):
        for arch in TARGETS:
            with self.subTest(arch=arch):
                records = self.records(arch)
                out = self.directory / f"good-{arch}"
                result = self.package(arch, records, out)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                archive = out / f"winlex-{arch}.zip"
                digest = hashlib.sha256(archive.read_bytes()).hexdigest()
                self.assertEqual((out / f"winlex-{arch}.zip.sha256").read_text().strip(),
                                 f"{digest}  {archive.name}")
                with ZipFile(archive) as z:
                    files = {entry.filename for entry in z.infolist() if not entry.is_dir()}
                    self.assertTrue({"winlex.exe", "winlex-gui.exe", "LICENSE", "THIRD-PARTY-NOTICES.md",
                                     "SHA256SUMS.txt", "licenses/YY-Thunks-LICENSE",
                                     "licenses/VC-LTL-LICENSE", "licenses/VC-LTL-sdk-license.rtf",
                                     "licenses/VC-LTL-sdk-third-party-notices.rtf",
                                     "vendor/PATCHES.md", "vendor/windows-metadata/license-mit",
                                     "vendor/windows-metadata/license-apache-2.0",
                                     "crates/winlex-gui/res/codicons/LICENSE"} <= files)
                    self.assertFalse(any(name.lower().endswith((".winmd", ".build.json")) for name in files))
                    lines = z.read("SHA256SUMS.txt").decode("utf-8").splitlines()
                    manifest = {}
                    for line in lines:
                        match = re.fullmatch(r"([0-9a-f]{64})  (.+)", line)
                        self.assertIsNotNone(match, line)
                        sha, name = match.groups()
                        self.assertNotIn(name, manifest)
                        manifest[name] = sha
                    self.assertEqual(set(manifest), files - {"SHA256SUMS.txt"})
                    for name, sha in manifest.items():
                        data = z.read(name)
                        self.assertEqual(hashlib.sha256(data).hexdigest(), sha, name)
                        for path in (str(ROOT), str(self.directory)):
                            for spelling in {path, path.replace("\\", "/")}:
                                self.assertNotIn(spelling.lower().encode(), data.lower(), name)
                                self.assertNotIn(spelling.lower().encode("utf-16le"), data.lower(), name)
                        if not name.lower().endswith(".exe") and os.environ.get("USERPROFILE"):
                            profile = os.environ["USERPROFILE"]
                            for spelling in {profile, profile.replace("\\", "/")}:
                                self.assertNotIn(spelling.lower().encode(), data.lower(), name)
                                self.assertNotIn(spelling.lower().encode("utf-16le"), data.lower(), name)
                for record, name in zip(records, ("winlex.exe", "winlex-gui.exe")):
                    with ZipFile(archive) as z:
                        self.assertEqual(hashlib.sha256(z.read(name)).hexdigest(),
                                         json.loads(record.read_text(encoding="utf-8-sig"))["executable_sha256"].lower())

    def test_rejects_bad_records_and_existing_outputs(self):
        records = self.records("x64")
        original = json.loads(records[0].read_text(encoding="utf-8-sig"))
        for field, value, message in (
            ("target", TARGETS["x86"], "target or configuration mismatch"),
            ("configuration", "Dist", "target or configuration mismatch"),
            ("executable_sha256", "0" * 64, "name or hash mismatch"),
        ):
            with self.subTest(field=field):
                bad = self.directory / f"bad-{field}.json"
                bad.write_text(json.dumps({**original, field: value}), encoding="utf-8")
                out = self.directory / f"bad-{field}"
                result = self.package("x64", [bad, records[1]], out)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn(message, result.stdout + result.stderr)
                self.assertFalse((out / "winlex-x64.zip").exists())
        for suffix in (".zip", ".zip.sha256"):
            with self.subTest(existing=suffix):
                out = self.directory / ("existing-" + suffix.replace(".", "-"))
                out.mkdir()
                existing = out / f"winlex-x64{suffix}"
                existing.write_bytes(b"keep")
                result = self.package("x64", records, out)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("Output already exists", result.stdout + result.stderr)
                self.assertEqual(existing.read_bytes(), b"keep")


class WorkflowContracts(unittest.TestCase):
    def test_manual_matrix_and_pinned_actions(self):
        try:
            import yaml
        except ImportError:
            self.skipTest("PyYAML is required for workflow YAML validation")
        workflow = yaml.load((ROOT / ".github/workflows/build.yml").read_text(encoding="utf-8"),
                             Loader=yaml.BaseLoader)
        self.assertEqual(set(workflow["on"]), {"workflow_dispatch"})
        job = workflow["jobs"]["dist"]
        matrix = job["strategy"]["matrix"]["include"]
        self.assertEqual({item["arch"]: item["target"] for item in matrix}, {
            **TARGETS, "arm64": "aarch64-pc-windows-msvc",
        })
        self.assertEqual(len(matrix), 3)
        actions = [step["uses"] for step in job["steps"] if "uses" in step]
        self.assertEqual(set(actions), {
            "actions/checkout@11d5960a326750d5838078e36cf38b85af677262",
            "actions/upload-artifact@ea165f8d65b6e75b540449e92b4886f43607fa02",
        })
        self.assertTrue(all(re.fullmatch(r"[^@]+@[0-9a-f]{40}", action) for action in actions))
        for step in job["steps"]:
            if "run" not in step:
                continue
            script = re.sub(r"\$\{\{\s*matrix\.target\s*\}\}", "x86_64-pc-windows-msvc", step["run"])
            script = re.sub(r"\$\{\{\s*matrix\.arch\s*\}\}", "x64", script)
            result = subprocess.run(["pwsh", "-NoProfile", "-Command",
                "$text=[Console]::In.ReadToEnd(); $errors=$null; "
                "[System.Management.Automation.Language.Parser]::ParseInput($text,[ref]$null,[ref]$errors) | Out-Null; "
                "if ($errors) { $errors | ForEach-Object { $_.Message }; exit 1 }"],
                input=script, text=True, capture_output=True, encoding="utf-8")
            self.assertEqual(result.returncode, 0, step.get("name", "run") + result.stdout + result.stderr)


if __name__ == "__main__":
    unittest.main()
