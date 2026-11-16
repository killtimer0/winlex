"""Runner contracts: bounded processes and independent gate continuation."""
import contextlib
import hashlib
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

import artifacts
import run as runner


class RunnerContracts(unittest.TestCase):
    def test_failed_and_timed_out_processes_allow_the_next_case(self):
        with tempfile.TemporaryDirectory(prefix="winlex runner ") as folder:
            marker = Path(folder) / "continued.txt"
            with contextlib.redirect_stderr(io.StringIO()):
                self.assertFalse(runner.run([sys.executable, "-c", "raise SystemExit(7)"], timeout=10))
                self.assertFalse(runner.run([sys.executable, "-c", "import time; time.sleep(30)"], timeout=0.1))
                self.assertTrue(runner.run([sys.executable, "-c",
                                           "from pathlib import Path; import sys; Path(sys.argv[1]).touch()",
                                           marker], timeout=10))
            self.assertTrue(marker.is_file())

    def test_build_timeout_retains_partial_output(self):
        real_run = subprocess.run

        def fake_cargo(command, **kwargs):
            return real_run([sys.executable, "-c",
                             "import time; print('build evidence', flush=True); time.sleep(30)"], **kwargs)

        output = io.StringIO()
        with patch("artifacts.subprocess.run", side_effect=fake_cargo), contextlib.redirect_stderr(output):
            with self.assertRaises(subprocess.TimeoutExpired):
                artifacts.build_example("winlex-cli", "host-cli", [], timeout=1)
        self.assertIn("build evidence", output.getvalue())

    def test_invalid_inputs_fail_before_building(self):
        script = Path(runner.__file__).resolve()
        for arguments in (["--winrt-apps", "missing"],
                          ["--winrt-sdk", "missing", "--winrt-apps", "missing"],
                          ["--gate-timeout", "0"], ["--build-timeout", "nan"]):
            with self.subTest(arguments=arguments):
                result = subprocess.run([sys.executable, str(script), *arguments],
                                        capture_output=True, text=True, timeout=10)
                self.assertEqual(result.returncode, 2)
                self.assertIn("error:", result.stderr)

    def run_round(self, root, failed_build=None):
        metadata = root / "tests/metadata"
        metadata.mkdir(parents=True)
        (root / "tests/abi").mkdir()
        (root / "inputs").mkdir()
        (root / "inputs/test.winmd").write_bytes(b"fixture")
        (metadata / "source.json").write_text(json.dumps({
            "file": "test.winmd", "sha256": hashlib.sha256(b"fixture").hexdigest()}))
        for name, path, status in (("fixture", metadata / "fixture_gate.py", 1),
                                   ("multisource", metadata / "multisource_gate.py", 0),
                                   ("ABI", root / "tests/abi/run.py", 0),
                                   ("WinRT", metadata / "winrt_gate.py", 0)):
            path.write_text(f"from pathlib import Path\nPath({name!r}).touch()\nraise SystemExit({status})\n")
        manifest = root / "artifacts.json"
        artifacts.write_manifest({"host-cli": root / "stale.exe"}, manifest)

        def build(package, name, options, timeout):
            if name == failed_build:
                raise subprocess.CalledProcessError(1, ["cargo", "build"])
            return Path(sys.executable)

        real_run = runner.run

        def run(command, timeout):
            if command[0] == "cargo":
                command = [sys.executable, "-c", "raise SystemExit(1)"]
            return real_run(command, timeout=timeout)

        output = io.StringIO()
        with patch.object(runner, "ROOT", root), patch.object(runner, "OUT", root / "out"), \
                patch.object(runner, "__file__", str(metadata / "run.py")), \
                patch.object(runner, "build_example", side_effect=build), \
                patch.object(runner, "run", side_effect=run), \
                patch.object(sys, "argv", ["run.py", "--abi", "--winrt-sdk", str(root),
                                           "--artifacts", str(manifest)]), \
                patch.dict(os.environ), contextlib.redirect_stderr(output):
            status = runner.main()
        return status, json.loads(manifest.read_text())["executables"]

    def test_cargo_and_gate_failures_do_not_stop_independent_gates(self):
        with tempfile.TemporaryDirectory(prefix="winlex runner ") as folder:
            root = Path(folder)
            status, _ = self.run_round(root)
            self.assertEqual(status, 1)
            for marker in ("fixture", "multisource", "ABI", "WinRT"):
                self.assertTrue((root / marker).is_file(), marker)

    def test_failed_build_skips_only_dependent_gates_and_removes_stale_artifact(self):
        for failed_build in ("host-cli", "query-audit"):
            with self.subTest(build=failed_build), tempfile.TemporaryDirectory(prefix="winlex runner ") as folder:
                root = Path(folder)
                status, manifest = self.run_round(root, failed_build)
                self.assertEqual(status, 1)
                self.assertNotIn(failed_build, manifest)
                self.assertEqual((root / "ABI").exists(), failed_build != "host-cli")
                self.assertFalse((root / "WinRT").exists())
                self.assertEqual((root / "multisource").exists(), failed_build != "host-cli")


if __name__ == "__main__":
    unittest.main()
