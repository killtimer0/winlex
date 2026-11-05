"""Standard-library contracts for Cargo output and standalone gate selection."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest
from unittest.mock import patch

from artifacts import (OVERRIDES, build_example, executable, manifest_path,
                       select_executable, write_manifest)


def artifact(path, name="host-cli", kind="example", test=False):
    return {"reason": "compiler-artifact", "target": {"name": name, "kind": [kind]},
            "profile": {"test": test}, "executable": str(path) if path else None}


class ArtifactContracts(unittest.TestCase):
    def test_selects_reported_executable_not_target_layout(self):
        path = Path.cwd() / "custom output/configured target/debug/examples/host-cli.exe"
        messages = [{"reason": "build-finished", "success": True},
                    artifact("wrong.exe", kind="bin"), artifact("test.exe", test=True),
                    artifact("other.exe", name="other-example"), artifact(None), artifact(path)]
        self.assertEqual(select_executable(messages, "host-cli"), path)

    def test_missing_and_ambiguous_outputs_fail(self):
        for messages in ([], [artifact("one.exe"), artifact("two.exe")]):
            with self.subTest(messages=messages), self.assertRaises(RuntimeError):
                select_executable(messages, "host-cli")

    def test_duplicate_reports_of_same_executable_are_unambiguous(self):
        message = artifact("one.exe")
        self.assertEqual(select_executable([message, message], "host-cli"), Path.cwd() / "one.exe")

    def test_build_preserves_spaces_and_defers_target_configuration_to_cargo(self):
        output = Path.cwd() / "configured target/custom profile/host-cli.exe"
        result = subprocess.CompletedProcess([], 0, json.dumps(artifact(output)) + "\n")
        with patch("artifacts.subprocess.run", return_value=result) as run:
            self.assertEqual(build_example("winlex-cli", "host-cli", ["--profile", "dev"]), output)
            command = run.call_args.args[0]
            self.assertNotIn("--target", command)
            self.assertNotIn("--target-dir", command)
            self.assertNotIn("--release", command)
            build_example("winlex-cli", "host-cli", ["--profile", "dev", "--target-dir", "output with spaces"])
            self.assertEqual(run.call_args.args[0][-2:], ["--target-dir", "output with spaces"])

    def test_failed_build_cannot_select_reported_artifact(self):
        result = subprocess.CompletedProcess([], 1, json.dumps(artifact("stale.exe")))
        with patch("artifacts.subprocess.run", return_value=result), self.assertRaises(subprocess.CalledProcessError):
            build_example("winlex-cli", "host-cli", [])

    def test_persisted_manifest_and_explicit_overrides(self):
        with tempfile.TemporaryDirectory(prefix="winlex artifacts ") as folder:
            root = Path(folder)
            manifest = root / "gate paths.json"
            reported = root / "cargo output.exe"
            override = root / "explicit override.exe"
            reported.touch()
            override.touch()
            write_manifest({name: reported for name in OVERRIDES}, manifest)
            with patch.dict(os.environ, {"WINLEX_GATE_ARTIFACTS": str(manifest)}, clear=True):
                for name, variable in OVERRIDES.items():
                    with self.subTest(name=name):
                        self.assertEqual(executable(name), reported)
                        with patch.dict(os.environ, {variable: str(override)}):
                            self.assertEqual(executable(name), override)
                reported.unlink()
                with self.assertRaisesRegex(RuntimeError, "does not exist"):
                    executable("host-cli")
                manifest.unlink()
                with self.assertRaisesRegex(RuntimeError, "run tests/metadata/run.py"):
                    executable("host-cli")
                with patch.dict(os.environ, {"WINLEX_GATE_CLI": str(override)}):
                    self.assertEqual(executable("host-cli"), override)

    def test_manifest_relative_paths_are_relative_to_manifest(self):
        with tempfile.TemporaryDirectory(prefix="winlex artifacts ") as folder:
            root = Path(folder)
            binary = root / "cli.exe"
            binary.touch()
            manifest = root / "artifacts.json"
            manifest.write_text(json.dumps({"version": 1, "executables": {"host-cli": "cli.exe"}}), encoding="utf-8")
            with patch.dict(os.environ, {"WINLEX_GATE_ARTIFACTS": str(manifest)}, clear=True):
                self.assertEqual(executable("host-cli"), binary)
                manifest.write_text('{"version": 2}', encoding="utf-8")
                with self.assertRaisesRegex(RuntimeError, "unsupported manifest version"):
                    executable("host-cli")

    def test_non_ascii_manifest_and_executable_paths(self):
        with tempfile.TemporaryDirectory(prefix="winlex 元数据 ") as folder:
            root = Path(folder)
            binary = root / "读取器 é.exe"
            override = root / "显式覆盖 ü.exe"
            manifest = root / "产物清单.json"
            binary.touch()
            override.touch()
            selected = select_executable([artifact(binary)], "host-cli")
            self.assertEqual(selected, binary)
            write_manifest({"host-cli": selected}, manifest)
            self.assertEqual(json.loads(manifest.read_text(encoding="utf-8"))["executables"]["host-cli"], str(binary))
            with patch.dict(os.environ, {"WINLEX_GATE_ARTIFACTS": str(manifest)}, clear=True):
                self.assertEqual(executable("host-cli"), binary)
                with patch.dict(os.environ, {"WINLEX_GATE_CLI": str(override)}):
                    self.assertEqual(executable("host-cli"), override)

    @unittest.skipUnless(os.name == "nt", "Windows extended paths require Windows filesystem APIs")
    def test_windows_extended_long_manifest_and_executable_paths(self):
        with tempfile.TemporaryDirectory(prefix="winlex long paths ") as folder:
            base = str(Path(folder).resolve())
            if not base.startswith("\\\\?\\"):
                base = "\\\\?\\UNC\\" + base[2:] if base.startswith("\\\\") else "\\\\?\\" + base
            tree = Path(base) / "长路径"
            root = tree
            for _ in range(5):
                root /= "元数据" + "x" * 60
            try:
                root.mkdir(parents=True)
                binary = root / "读取器.exe"
                manifest = root / "产物.json"
                self.assertGreater(len(str(binary)), 260)
                self.assertTrue(str(binary).startswith("\\\\?\\"))
                binary.write_bytes(b"path contract fixture")
                selected = select_executable([artifact(binary)], "host-cli")
                self.assertEqual(selected, binary)
                write_manifest({"host-cli": selected}, manifest)
                self.assertEqual(json.loads(manifest.read_text(encoding="utf-8"))["executables"]["host-cli"], str(binary))
                with patch.dict(os.environ, {"WINLEX_GATE_ARTIFACTS": str(manifest)}, clear=True):
                    self.assertEqual(str(manifest_path()), str(manifest))
                    self.assertEqual(executable("host-cli"), binary)
                    self.assertEqual(executable("host-cli").read_bytes(), b"path contract fixture")
                    with patch.dict(os.environ, {"WINLEX_GATE_CLI": str(binary)}):
                        self.assertEqual(executable("host-cli"), binary)
                manifest.write_text(json.dumps({"version": 1, "executables": {"host-cli": binary.name}}), encoding="utf-8")
                with patch.dict(os.environ, {"WINLEX_GATE_ARTIFACTS": str(manifest)}, clear=True):
                    self.assertEqual(executable("host-cli"), binary)
            finally:
                if tree.exists():
                    shutil.rmtree(tree)

    @unittest.skipUnless(os.name == "nt", "Local Windows administrative shares require Windows")
    def test_unc_manifest_and_executable_paths_on_actual_admin_share(self):
        with tempfile.TemporaryDirectory(prefix="winlex UNC 元数据 ") as folder:
            local = Path(folder).resolve()
            if len(local.drive) != 2 or local.drive[1] != ":":
                self.skipTest("Temporary directory is not on a drive with a local administrative share")
            binary = local / "读取器.exe"
            binary.write_bytes(b"UNC path contract fixture")
            unc = Path("\\\\localhost\\" + local.drive[0] + "$" + str(local)[2:])
            unc_binary = unc / binary.name
            try:
                contents = unc_binary.read_bytes()
            except OSError as error:
                self.skipTest(f"Local administrative share unavailable: {error}")
            self.assertEqual(contents, b"UNC path contract fixture")
            selected = select_executable([artifact(unc_binary)], "host-cli")
            self.assertEqual(selected, unc_binary)
            self.assertTrue(str(selected).startswith("\\\\"))
            manifest = unc / "产物.json"
            write_manifest({"host-cli": selected}, manifest)
            self.assertEqual(json.loads((local / manifest.name).read_text(encoding="utf-8"))["executables"]["host-cli"], str(unc_binary))
            with patch.dict(os.environ, {"WINLEX_GATE_ARTIFACTS": str(manifest)}, clear=True):
                self.assertEqual(executable("host-cli"), unc_binary)
                self.assertEqual(executable("host-cli").read_bytes(), contents)
                with patch.dict(os.environ, {"WINLEX_GATE_CLI": str(unc_binary)}):
                    self.assertEqual(executable("host-cli"), unc_binary)


if __name__ == "__main__":
    unittest.main()
