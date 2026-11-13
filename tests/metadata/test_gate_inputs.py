"""Standalone WinRT gate argument contracts without installed SDKs or builds."""
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

import fixture_gate
import multisource_gate
import winrt_gate


GATE = Path(__file__).with_name("winrt_gate.py")


class WinrtInputContracts(unittest.TestCase):
    def run_gate(self, sdk, *options):
        env = os.environ.copy()
        env["WINLEX_GATE_ARTIFACTS"] = str(sdk / "missing-artifacts.json")
        return subprocess.run([sys.executable, str(GATE), "--sdk", str(sdk), *options],
                              capture_output=True, text=True, encoding="utf-8",
                              env=env, timeout=30)

    def test_explicit_apps_without_metadata_fails_before_sdk_or_artifacts(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            empty = root / "empty"
            empty.mkdir()
            file = root / "not-a-directory.winmd"
            file.touch()
            for apps in (root / "missing", empty, file, root / "*.winmd"):
                with self.subTest(apps=apps):
                    result = self.run_gate(root / "missing-sdk", "--apps", str(apps))
                    self.assertEqual(result.returncode, 2, result.stderr)
                    self.assertIn("--apps", result.stderr)
                    self.assertIn("no metadata/*.winmd inputs found", result.stderr)
                    self.assertNotIn("Traceback", result.stderr)

    def test_omitted_apps_is_accepted_and_reaches_sdk_validation(self):
        with tempfile.TemporaryDirectory() as folder:
            result = self.run_gate(Path(folder) / "missing-sdk")
            self.assertNotEqual(result.returncode, 0)
            self.assertNotEqual(result.returncode, 2, result.stderr)
            self.assertNotIn("no metadata/*.winmd inputs found", result.stderr)

    def test_audit_budget_must_be_finite_and_positive(self):
        with tempfile.TemporaryDirectory() as folder:
            for value in ("0", "-1", "nan", "inf"):
                with self.subTest(value=value):
                    result = self.run_gate(Path(folder), "--audit-timeout", value)
                    self.assertEqual(result.returncode, 2, result.stderr)
                    self.assertIn("finite and positive", result.stderr)


    def test_failed_run_does_not_leave_previous_success_report(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            report = root / "report.json"
            report.write_text('{"passed": true}')
            with patch.object(winrt_gate, "OUT", root), \
                 patch.object(sys, "argv", [str(GATE), "--sdk", str(root / "missing-sdk")]):
                with self.assertRaises(AssertionError):
                    winrt_gate.main()
            self.assertFalse(report.exists())


class ReferenceIdentityContracts(unittest.TestCase):
    def test_windows_references_preserve_source_and_token_in_cli_identifiers(self):
        # AppInstance's real JSON reference shape, plus the same token in another source.
        captured = {
            "arguments": [{"arguments": [], "kind": "named",
                           "name": "AppActivationArguments", "namespace": "Microsoft.Windows.AppLifecycle",
                           "reference_id": {"source_id": 14, "token": 16777219}}],
            "kind": "named", "name": "EventHandler`1", "namespace": "Windows.Foundation",
            "reference_id": {"source_id": 14, "token": 16777232},
        }
        other_source = {**captured, "reference_id": {"source_id": 15, "token": 16777232}}
        refs = winrt_gate.windows_reference_ids([captured, captured, other_source])
        self.assertEqual(set(refs), {"14::0x01000010", "15::0x01000010"})
        self.assertEqual(refs["14::0x01000010"], captured)
        self.assertEqual(refs["15::0x01000010"], other_source)


class QueryTimeoutContracts(unittest.TestCase):
    def test_single_query_timeout_propagates_with_inputs_and_stderr(self):
        for gate in (fixture_gate, multisource_gate, winrt_gate):
            with self.subTest(gate=gate.__name__):
                command = ["host-cli.exe", "input.winmd", "layout", "Example"]
                timeout = subprocess.TimeoutExpired(command, 30, stderr=b"reader stalled")
                with patch.object(gate, "executable", return_value=Path(command[0])), \
                     patch.object(gate.subprocess, "run", side_effect=timeout) as run:
                    with self.assertRaises(subprocess.TimeoutExpired) as raised:
                        if gate is fixture_gate:
                            gate.layout(Path("input.winmd"), "Example")
                        else:
                            gate.query([Path("input.winmd")], "layout", "Example")
                self.assertIs(raised.exception, timeout)
                self.assertEqual(run.call_args.kwargs["timeout"], 30)
                diagnostics = "\n".join(timeout.__notes__)
                self.assertIn("input.winmd", diagnostics)
                self.assertIn("reader stalled", diagnostics)


if __name__ == "__main__":
    unittest.main()
