"""Packaging contracts using explicitly selected or standard Debug build records."""
import hashlib
import json
import os
import re
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path
from zipfile import ZipFile


ROOT = Path(__file__).resolve().parents[2]
TARGETS = {"x86": "i686-pc-windows-msvc", "x64": "x86_64-pc-windows-msvc", "arm64": "aarch64-pc-windows-msvc"}
ARCHITECTURES = [os.environ["WINLEX_PACKAGE_REQUIRE_ARCH"]] if os.environ.get("WINLEX_PACKAGE_REQUIRE_ARCH") else list(TARGETS)
SCRIPT = ROOT / "scripts/package.ps1"


class PackageContracts(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        (ROOT / "tmp").mkdir(exist_ok=True)
        cls.temp = tempfile.TemporaryDirectory(prefix="打包 [contract]-", dir=ROOT / "tmp")
        cls.directory = Path(cls.temp.name)

    @classmethod
    def tearDownClass(cls):
        cls.temp.cleanup()

    def records(self, arch):
        binary_dir = Path(os.environ.get(f"WINLEX_PACKAGE_{arch.upper()}_BUILD_DIR",
                                        ROOT / "target" / TARGETS[arch] / "debug"))
        records = [binary_dir / f"{name}.exe.build.json" for name in ("winlex", "winlex-gui")]
        if not all(path.is_file() for path in records):
            if os.environ.get("WINLEX_PACKAGE_REQUIRE_ARCH") == arch:
                self.fail(f"Required Debug build records not available: {binary_dir}")
            self.skipTest(f"Debug build records not available: {binary_dir}")
        return records

    def package(self, arch, records, output, configuration="Debug"):
        return subprocess.run([
            "pwsh", "-NoProfile", "-File", str(SCRIPT), "-Architecture", arch,
            "-Configuration", configuration, "-CliBuildRecord", str(records[0]),
            "-GuiBuildRecord", str(records[1]), "-OutputDir", str(output),
        ], text=True, capture_output=True, encoding="utf-8", errors="replace")

    def test_debug_packages(self):
        for arch in ARCHITECTURES:
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
                                      "licenses/win32-darkmodelib-MPL-2.0.md",
                                      "licenses/win32-darkmodelib-UAHMenuBar.md",
                                      "licenses/win32-darkmode-MIT.md",
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
        arch = ARCHITECTURES[0] if os.environ.get("WINLEX_PACKAGE_REQUIRE_ARCH") else "x64"
        records = self.records(arch)
        original = json.loads(records[0].read_text(encoding="utf-8-sig"))
        for field, value, message in (
            ("policy", {**original["policy"], "target": "unsupported-target"}, "target or configuration mismatch"),
            ("configuration", "Dist", "target or configuration mismatch"),
            ("executable_sha256", "0" * 64, "name or hash mismatch"),
            ("schema_version", 0, "Unsupported build record"),
            ("policy", {**original["policy"], "minimum_windows": "different"}, "policy mismatch"),
        ):
            with self.subTest(field=field):
                bad = self.directory / f"bad-{field}.json"
                bad.write_text(json.dumps({**original, field: value}), encoding="utf-8")
                out = self.directory / f"bad-{field}"
                result = self.package(arch, [bad, records[1]], out)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn(message, result.stdout + result.stderr)
                self.assertFalse((out / f"winlex-{arch}.zip").exists())
        for suffix in (".zip", ".zip.sha256"):
            with self.subTest(existing=suffix):
                out = self.directory / ("existing-" + suffix.replace(".", "-"))
                out.mkdir()
                existing = out / f"winlex-{arch}{suffix}"
                existing.write_bytes(b"keep")
                result = self.package(arch, records, out)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("Output already exists", result.stdout + result.stderr)
                self.assertEqual(existing.read_bytes(), b"keep")

    def test_packages_unicode_executable_paths(self):
        arch = ARCHITECTURES[0] if os.environ.get("WINLEX_PACKAGE_REQUIRE_ARCH") else "x64"
        source = self.directory / "程序 [build]"
        source.mkdir()
        paths = []
        for path in self.records(arch):
            record = json.loads(path.read_text(encoding="utf-8-sig"))
            executable = source / Path(record["executable"]).name
            shutil.copyfile(record["executable"], executable)
            record["executable"] = str(executable)
            destination = source / path.name
            destination.write_text(json.dumps(record, ensure_ascii=False), encoding="utf-8")
            paths.append(destination)
        result = self.package(arch, paths, self.directory / "unicode-package")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_system_crt_imports_and_build_records(self):
        for arch in ARCHITECTURES:
            with self.subTest(arch=arch):
                for path in self.records(arch):
                    record = json.loads(path.read_text(encoding="utf-8-sig"))
                    arm64 = arch == "arm64"
                    self.assertEqual(record["policy"]["crt"], "ucrtbase.dll" if arm64 else "msvcrt.dll")
                    self.assertTrue(record["vc_ltl_root"])
                    self.assertTrue({"libucrt.lib", "libvcruntime.lib"} <=
                                    {lib["name"] for lib in record["vc_ltl_libraries"]})
                    self.assertEqual("thunks_object" in record, not arm64)
                    self.assertEqual("thunks_sha256" in record, not arm64)
                    dumpbin = Path(record["linker"]).with_name("dumpbin.exe")
                    result = subprocess.run([str(dumpbin), "/imports", record["executable"]],
                                            text=True, capture_output=True, errors="replace")
                    self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                    dlls = set(re.findall(r"^\s+([^\s]+\.dll)\s*$", result.stdout.lower(), re.MULTILINE))
                    self.assertIn("ucrtbase.dll" if arm64 else "msvcrt.dll", dlls)
                    self.assertFalse(any(name.startswith(("vcruntime", "msvcp", "concrt")) or
                                         name in ("ucrtbased.dll", "msvcr120.dll") for name in dlls), dlls)


class WorkflowContracts(unittest.TestCase):
    def workflow(self):
        try:
            import yaml
        except ImportError:
            self.skipTest("PyYAML is required for workflow YAML validation")
        return yaml.load((ROOT / ".github/workflows/build.yml").read_text(encoding="utf-8"),
                         Loader=yaml.BaseLoader)

    def test_manual_matrix_and_script_syntax(self):
        workflow = self.workflow()
        self.assertEqual(set(workflow["on"]), {"workflow_dispatch"})
        job = next(job for job in workflow["jobs"].values() if "matrix" in job.get("strategy", {}))
        self.assertEqual(set(job["strategy"]["matrix"]["arch"]), set(TARGETS))
        for step in (step for job in workflow["jobs"].values() for step in job["steps"]):
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
