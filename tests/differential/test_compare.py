import contextlib
import hashlib
import io
import json
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

from run import ROOT, compare_files, differences, discover, extract, main


class ComparisonContracts(unittest.TestCase):
    def test_detects_signature_and_constant_changes_at_entity_path(self):
        original = {"methods": {"0x06000001": {"parameter": {"kind": "pointer", "element": {"kind": "i32"}}}},
                    "constants": {"0x0b000001": {"value": "18446744073709551615"}}}
        changed = json.loads(json.dumps(original))
        changed["methods"]["0x06000001"]["parameter"]["element"]["kind"] = "u32"
        changed["constants"]["0x0b000001"]["value"] = "18446744073709551614"
        delta = list(differences(original, changed))
        self.assertEqual({d["path"] for d in delta}, {
            "$/methods/0x06000001/parameter/element/kind/", "$/constants/0x0b000001/value/"})

    def test_missing_null_and_boolean_are_not_equal(self):
        for a, b in (({}, {"x": None}), ([None], []), (True, 1)):
            with self.subTest(a=a, b=b):
                self.assertTrue(list(differences(a, b)))

    def test_discovery_is_recursive_and_rejects_empty_or_missing_inputs(self):
        with tempfile.TemporaryDirectory(prefix="winmd discovery ", dir=ROOT / "tmp") as temp:
            folder = Path(temp)
            with self.assertRaises(ValueError):
                discover(folder)
            with self.assertRaises(ValueError):
                discover(folder / "absent")
            (folder / "nested").mkdir()
            for name in ("a.winmd", "nested/a.WINMD", "ignore.dll"):
                (folder / name).touch()
            self.assertEqual([p.relative_to(folder).as_posix() for p in discover(folder)], ["a.winmd", "nested/a.WINMD"])

    def test_reader_failure_preserves_both_errors_and_continues_batch(self):
        with tempfile.TemporaryDirectory(prefix="winmd failure ", dir=ROOT / "tmp") as temp:
            folder = Path(temp)
            source = folder / "inputs"
            source.mkdir()
            for name in ("bad.winmd", "later.winmd"):
                (source / name).write_bytes(b"not a PE")
            output = folder / "evidence"
            output.mkdir()
            fail = [sys.executable, "-c", "import sys; print('parse failure', file=sys.stderr); sys.exit(7)"]
            with contextlib.redirect_stdout(io.StringIO()):
                report = compare_files(source, discover(source), output, fail, fail, 10)
            self.assertFalse(report["passed"])
            self.assertEqual([r["file"] for r in report["files"]], ["bad.winmd", "later.winmd"])
            for record in report["files"]:
                self.assertEqual(record["sha256"], hashlib.sha256(b"not a PE").hexdigest())
                self.assertEqual(set(record["errors"]), {"rust", "dotnet"})
                self.assertIn("parse failure", (output / record["evidence"] / "rust.stderr").read_text())

    def test_empty_directory_returns_failure(self):
        with tempfile.TemporaryDirectory(prefix="winmd empty ", dir=ROOT / "tmp") as temp:
            from unittest.mock import patch
            with patch.object(sys, "argv", ["run.py", temp]), contextlib.redirect_stderr(io.StringIO()):
                self.assertEqual(main(), 1)

    def test_invalid_output_and_timeout_are_failures(self):
        with tempfile.TemporaryDirectory(prefix="winmd reader errors ", dir=ROOT / "tmp") as temp:
            folder = Path(temp)
            source = folder / "input.winmd"
            source.touch()
            for name, command, timeout, error in (
                ("invalid", [sys.executable, "-c", "print('{}')"], 10, ValueError),
                ("timeout", [sys.executable, "-c", "import time; time.sleep(30)"], .05, subprocess.TimeoutExpired),
            ):
                output = folder / name
                output.mkdir()
                with self.subTest(case=name), self.assertRaises(error):
                    extract(command, source, output, "rust", timeout)


class IndependentReaders(unittest.TestCase):
    def test_real_fixture_and_corrupt_file(self):
        # Keep fixture sources, build products and comparison evidence for diagnosis.
        folder = Path(tempfile.mkdtemp(prefix="differential fixture ", dir=ROOT / "tmp"))
        build = folder / "build"
        with (folder / "fixture-build.log").open("wb") as log:
            subprocess.run(["dotnet", "build", ROOT / "tests/differential/Fixtures", "-c", "Debug", "-o", build],
                           stdout=log, stderr=subprocess.STDOUT, check=True)
        inputs = folder / "inputs"
        inputs.mkdir()
        shutil.copyfile(build / "Fixtures.dll", inputs / "fixture.winmd")
        evidence = folder / "comparison"
        result = subprocess.run([sys.executable, ROOT / "tests/differential/run.py", inputs, "--output", evidence],
                                capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr + f"\nEvidence: {folder}")
        facts = json.loads((evidence / "0000/rust.json").read_text())
        fields = {v["name"]: v for v in facts["fields"].values()}
        self.assertEqual(fields["PointerPointer"]["type"]["element"]["kind"], "pointer")
        self.assertEqual(fields["Second"]["offset"], 8)
        convert = next(v for v in facts["methods"].values() if v["name"] == "Convert")
        self.assertEqual(convert["signature"]["generic_arity"], 1)
        self.assertEqual(convert["signature"]["return"], {"kind": "mvar", "index": 0})
        self.assertEqual(convert["signature"]["parameters"][0], {"kind": "var", "index": 0})
        native_array = next(v for v in facts["attributes_decoded"].values() if v["name"] == "NativeArrayInfoAttribute")
        self.assertEqual(native_array["arguments"], [{"name": "CountConst", "value": "17"}, {"name": "CountParamIndex", "value": "2"}])
        alignment = next(v for v in facts["attributes_decoded"].values()
                         if v["namespace"] == "Winlex.Metadata" and v["name"] == "NativeAlignmentAttribute")
        self.assertEqual(alignment["arguments"], [{"name": "", "value": "16"}])
        self.assertIn("18446744073709551615", [v["value"] for v in facts["constants"].values()])
        (inputs / "broken.winmd").write_bytes(b"not a PE")
        failure = subprocess.run([sys.executable, ROOT / "tests/differential/run.py", inputs, "--output", folder / "failed"],
                                 capture_output=True, text=True)
        self.assertEqual(failure.returncode, 1, failure.stdout + failure.stderr)
        report = json.loads((folder / "failed/report.json").read_text())
        self.assertEqual([r["passed"] for r in report["files"]], [False, True])


if __name__ == "__main__":
    unittest.main()
