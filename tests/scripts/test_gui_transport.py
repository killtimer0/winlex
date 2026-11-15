"""Non-desktop contracts for the explicit GUI transport CLI and target config."""
import ast
import base64
import configparser
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


GUI = Path(__file__).resolve().parents[1] / "gui"
spec = importlib.util.spec_from_file_location("gui_transport", GUI / "run.py")
transport = importlib.util.module_from_spec(spec)
spec.loader.exec_module(transport)
execute_spec = importlib.util.spec_from_file_location("gui_execute", GUI / "execute.py")
execute = importlib.util.module_from_spec(execute_spec)
execute_spec.loader.exec_module(execute)


class GuiTransportContracts(unittest.TestCase):
    def cli(self, *arguments):
        return subprocess.run([sys.executable, str(GUI / "run.py"), *arguments],
                              capture_output=True, text=True, timeout=10)

    def test_help_is_available_without_target_or_desktop_access(self):
        result = self.cli("--help")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("--allow-local-desktop", result.stdout)
        self.assertIn("--config", result.stdout)

    def test_no_implicit_local_transport_and_no_unapproved_local_desktop(self):
        inputs = ["--exe", "unused.exe", "--winmd", "unused.winmd"]
        for options, expected in [([], "--local"), (["--local"], "--allow-local-desktop"),
                                  (["--config", "unused.json", "--allow-local-desktop"], "only valid")]:
            with self.subTest(options=options):
                result = self.cli(*options, *inputs)
                self.assertEqual(result.returncode, 2)
                self.assertIn(expected, result.stderr)

    def test_rejects_nonfinite_budget_and_overridden_target_paths_before_any_launch(self):
        inputs = ["--config", "unused.json", "--exe", "unused.exe", "--winmd", "unused.winmd"]
        for options in [("--timeout", "nan"), ("--timeout", "inf"), ("--timeout", "0"),
                        ("--", "--output=elsewhere"), ("--", "--exe", "other.exe"),
                        ("--", "--desktop-authorized"), ("--", "--junitxml=other.xml"),
                        ("--", "-c", "other.ini"), ("--", "--rootdir=elsewhere"),
                        ("--", "../other/test_external.py::test_case"),
                        ("--", "C:/other/test_external.py"), ("--", "../other"),
                        ("--", "C:/other")]:
            with self.subTest(options=options):
                result = self.cli(*inputs, *options)
                self.assertEqual(result.returncode, 2)
                self.assertNotIn("Traceback", result.stderr)
                self.assertNotIn("Local artifacts:", result.stdout)

    def test_pytest_selection_fault_and_input_options_are_forwarded_unchanged(self):
        options = ["-k", "space 中文", "--fault-mode", "driver-hang", "--real-input"]
        args = transport.parse_args(["--config", "target.json", "--exe", "gui.exe",
                                     "--winmd", "input.winmd", "--", *options])
        self.assertEqual(args.pytest_args, options)

    def test_job_probe_is_explicit_without_pytest_selection(self):
        args = transport.parse_args(["--config", "target.json", "--exe", "gui.exe",
                                     "--winmd", "input.winmd", "--probe-job"])
        self.assertTrue(args.probe_job)
        self.assertEqual(args.pytest_args, [])

    def test_suite_snapshot_includes_timeout_configuration(self):
        self.assertIn(GUI / "pytest.ini", transport.suite_files())
        ini = configparser.ConfigParser()
        ini.read(GUI / "pytest.ini", encoding="utf-8")
        self.assertEqual(ini.getfloat("pytest", "timeout"), 45)
        self.assertEqual(ini.get("pytest", "timeout_method"), "thread")

    def test_target_checks_every_deployed_hash_and_records_mismatch(self):
        with tempfile.TemporaryDirectory(prefix="gui-transport-") as folder:
            folder = Path(folder)
            source = folder / "space 中文.py"
            source.write_bytes(b"source snapshot\x00\xff")
            winmd = folder / "input.winmd"
            winmd.write_bytes(b"metadata")
            output = folder / "output"
            manifest = {"output": str(output),
                        "files": transport.input_hashes([(source, source), (winmd, winmd)])}
            execute.verify_deployment(manifest)
            audit = json.loads((output / "input-hashes.json").read_text(encoding="utf-8"))
            self.assertEqual(len(audit["files"]), 2)
            self.assertTrue(all(item["verified"] for item in audit["files"]))
            source.write_bytes(b"changed after snapshot")
            with self.assertRaisesRegex(RuntimeError, "SHA256 mismatch"):
                execute.verify_deployment(manifest)
            audit = json.loads((output / "input-hashes.json").read_text(encoding="utf-8"))
            self.assertFalse(audit["files"][0]["verified"])
            self.assertTrue(audit["files"][1]["verified"])

    def test_explicit_pytest_node_does_not_also_select_the_whole_suite(self):
        node = 'test_workflows.py::test_search_read_find_and_exit'
        args = transport.parse_args(['--local', '--allow-local-desktop', '--exe', 'gui.exe',
                                     '--winmd', 'input.winmd', '--', node])
        self.assertTrue(args.explicit_nodes)
        command = execute.pytest_command(dict(exe='gui.exe', winmd='input.winmd',
                                              output='evidence', pytest_args=[node], explicit_nodes=True), GUI)
        self.assertIn(node, command)
        self.assertNotIn(str(GUI), command)
        default = dict(exe='gui.exe', winmd='input.winmd', output='evidence', pytest_args=['-k', 'find'])
        self.assertIn(str(GUI), execute.pytest_command(default, GUI))

    def test_tail_cleanup_only_accepts_exact_private_uuid_registry_roots(self):
        prefix = "Software\\Winlex\\E2E\\"
        for suffix in ["a" * 32, "12345678-1234-1234-1234-123456789abc"]:
            self.assertEqual(execute.private_config_path(prefix + suffix), prefix + suffix)
        for value in [None, "Software\\Winlex", prefix, prefix + "g" * 32,
                      prefix + "a" * 32 + "\\child", prefix + "..\\" + "a" * 32,
                      "Software\\Other\\E2E\\" + "a" * 32]:
            with self.subTest(value=value), self.assertRaises(ValueError):
                execute.private_config_path(value)

    def test_cancellation_accepts_a_naturally_finished_result_without_execute_pid(self):
        with tempfile.TemporaryDirectory(prefix="gui-transport-") as folder:
            folder = Path(folder)
            output = folder / "output"
            output.mkdir()
            (folder / "manifest.json").write_text(json.dumps({"output": str(output)}), encoding="utf-8")
            result = {"exit_code": 0, "timed_out": False,
                      "cleanup": {"applications_exited": True, "configs_removed": True}}
            execute.write_result(output, result)
            self.assertEqual(execute.cancel_run(folder), result)
            self.assertFalse((folder / "cancel.requested").exists())

    @unittest.skipUnless(os.name == "nt", "Windows non-GUI execute CLI")
    def test_cancel_before_pid_blocks_late_execute_without_launching_pytest(self):
        with tempfile.TemporaryDirectory(prefix="gui-cancel-") as folder:
            folder = Path(folder)
            output = folder / "output"
            output.mkdir()
            manifest = folder / "manifest.json"
            # No EXE, metadata or deployment files: cancelled execution must not
            # reach dependency verification, desktop inspection or child launch.
            manifest.write_text(json.dumps({"output": str(output), "desktop_authorized": True,
                                            "timeout": 10}), encoding="utf-8")
            with self.assertRaisesRegex(RuntimeError, "launch/cleanup is unknown"):
                execute.cancel_run(folder)
            self.assertTrue((folder / "cancel.requested").is_file())
            self.assertFalse((output / "result.json").exists())
            completed = subprocess.run(
                [sys.executable, str(GUI / "execute.py"), "--manifest", str(manifest)],
                stdin=subprocess.DEVNULL, capture_output=True, text=True, timeout=10,
                creationflags=subprocess.CREATE_NO_WINDOW)
            self.assertEqual(completed.returncode, 130, completed.stderr)
            result = json.loads((output / "result.json").read_text(encoding="utf-8"))
            self.assertTrue(result["cancelled"])
            self.assertTrue(result["cleanup"]["pytest_exited"])
            self.assertTrue(result["cleanup"]["applications_exited"])
            self.assertNotIn("error", result)
            self.assertFalse((output / "execution-environment.json").exists())
            self.assertFalse((output / "job-capability.json").exists())
            self.assertFalse((output / "junit.xml").exists())

    @unittest.skipUnless(os.name == "nt", "Windows process identity contract")
    def test_application_check_distinguishes_live_dead_and_reused_pid_without_killing(self):
        child = subprocess.Popen([sys.executable, "-c", "import time;time.sleep(30)"],
                                 stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                                 stderr=subprocess.DEVNULL, creationflags=subprocess.CREATE_NO_WINDOW)
        try:
            created = execute.process_identity(child.pid)
            self.assertFalse(execute.application_exited(child.pid, created))
            self.assertTrue(execute.application_exited(child.pid, created + 1))
            self.assertIsNone(child.poll())
            child.terminate()
            child.wait(timeout=10)
            self.assertTrue(execute.application_exited(child.pid, created))
        finally:
            if child.poll() is None:
                child.terminate()
                child.wait(timeout=10)

    def config(self, folder, **updates):
        key = folder / "private-key"
        key.touch()
        value = {"host": "tester@win7", "python": "C:/Program Files/Python38/python.exe",
                 "root": "C:/GUI tests/中文", "sftp_drive_root": "/", "key_file": str(key)}
        value.update(updates)
        path = folder / "target.json"
        path.write_text(json.dumps(value), encoding="utf-8")
        return path

    def test_ssh_config_rejects_local_transport_options_and_unsafe_paths(self):
        with tempfile.TemporaryDirectory() as folder:
            for updates in [{"transport": "local"}, {"host": "-oProxyCommand=bad"},
                            {"host": "host\nput file"}, {"host": "host extra"},
                            {"root": "relative"}, {"python": "python.exe"},
                            {"python": "C:/%TEMP%/python.exe"}, {"python": 'C:/a"/python.exe'},
                            {"port": True}, {"port": 0}, {"key_file": "missing-private-key"}]:
                with self.subTest(updates=updates):
                    path = self.config(Path(folder), **updates)
                    with self.assertRaises(ValueError):
                        transport.load_config(path)

    def test_existing_config_fields_do_not_select_a_local_fallback(self):
        with tempfile.TemporaryDirectory() as folder:
            config = transport.load_config(self.config(Path(folder), script="old.py", desktop="isolated"))
            self.assertEqual(config["host"], "tester@win7")
            self.assertEqual(transport.sftp_path("C:/GUI tests/中文/run/output", config),
                             "/C/GUI tests/中文/run/output")
            self.assertEqual(transport.sftp_path("\\\\?\\C:\\GUI tests\\中文", config),
                             "/C/GUI tests/中文")
            with self.assertRaises(ValueError):
                transport.sftp_path("\\\\server\\share\\run", config)

    def test_ssh_config_requires_host_key_python_and_root(self):
        with tempfile.TemporaryDirectory() as folder:
            for field in ("host", "key_file", "python", "root"):
                with self.subTest(field=field):
                    path = self.config(Path(folder))
                    value = json.loads(path.read_text(encoding="utf-8"))
                    del value[field]
                    path.write_text(json.dumps(value), encoding="utf-8")
                    with self.assertRaisesRegex(ValueError, field):
                        transport.load_config(path)

    def test_native_sftp_paths_preserve_unc_and_batch_paths_cannot_add_commands(self):
        self.assertEqual(transport.sftp_path("\\\\server\\share\\中文 file", {}),
                         "//server/share/中文 file")
        self.assertEqual(transport.sftp_quote('C:/space 中文/file'), '"C:/space 中文/file"')
        for path in ["file\n!command", "file\rput other", "file\x00"]:
            with self.assertRaises(ValueError):
                transport.sftp_quote(path)

    def test_bitvise_command_encodes_source_and_quotes_executable_and_entire_command(self):
        source = "print('中文 & %PATH%')\n"
        command = transport.python_command("C:/Program Files/Python38/python.exe", source)
        self.assertTrue(command.startswith('\"\"C:/Program Files/Python38/python.exe\" -c \"'))
        self.assertTrue(command.endswith('\"\"'))
        encoded = command.split("b64decode('", 1)[1].split("')", 1)[0]
        self.assertEqual(base64.b64decode(encoded).decode("utf-8"), source)

    def test_target_runner_syntax_is_python38_compatible(self):
        for path in (GUI / "run.py", GUI / "execute.py"):
            with self.subTest(path=path):
                ast.parse(path.read_text(encoding="utf-8"), feature_version=(3, 8))
        for source in (transport.PREFLIGHT, transport.LEGACY_SCHEDULER):
            ast.parse(source, feature_version=(3, 8))


if __name__ == "__main__":
    unittest.main()
