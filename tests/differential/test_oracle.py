import json
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

from run import ROOT


class ExternalEnumOracle(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        (ROOT / "tmp/oracle-ucrt-check").mkdir(parents=True, exist_ok=True)
        cls.folder = Path(tempfile.mkdtemp(prefix="enum-", dir=ROOT / "tmp/oracle-ucrt-check"))
        cls.oracle = cls.folder / "oracle"
        cls.fixture = cls.folder / "fixture"
        for project, output, name in (
            ("Oracle", cls.oracle, "oracle"),
            ("EnumFixtures/Consumer", cls.fixture, "fixture"),
            ("EnumFixtures/Wide", cls.folder / "wrong", "wrong-identity"),
        ):
            command = ["dotnet", "build", ROOT / "tests/differential" / project,
                       "-c", "Debug", "-o", output]
            if name == "wrong-identity":
                command.append("-p:AssemblyVersion=9.8.7.6")
            with (cls.folder / f"{name}-build.log").open("wb") as log:
                subprocess.run(command, cwd=ROOT, stdout=log, stderr=subprocess.STDOUT, check=True, timeout=300)

    def decode(self, name, dependencies=(), sibling=False):
        work = self.folder / name
        work.mkdir()
        source = work / "Consumer.winmd"
        shutil.copyfile(self.fixture / "Consumer.dll", source)
        if sibling:
            for dependency in dependencies:
                shutil.copyfile(dependency, work / dependency.name)
            dependencies = ()
        output = work / "facts.json"
        result = subprocess.run(["dotnet", self.oracle / "Oracle.dll", source, output, *dependencies],
                                cwd=ROOT, capture_output=True, text=True, timeout=180)
        (work / "stderr.txt").write_text(result.stderr, encoding="utf-8")
        return result, output

    def test_external_enum_values_and_same_type_name_use_assembly_scope(self):
        for sibling in (False, True):
            with self.subTest(sibling=sibling):
                result, output = self.decode(f"values-{sibling}",
                    [self.fixture / "Wide.dll", self.fixture / "Narrow.dll"], sibling=sibling)
                self.assertEqual(result.returncode, 0, result.stderr)
                attributes = json.loads(output.read_text())["attributes_decoded"].values()
                values = {a["name"]: a["arguments"] for a in attributes}
                # Values are fixed by the C# enum declarations, not another reader's output.
                self.assertEqual(values["SupportedArchitectureAttribute"],
                                 [{"name": "", "value": str(0xfedcba9876543210)}])
                self.assertEqual(values["NativeArrayInfoAttribute"], [{"name": "", "value": "-12345"}])

    def test_missing_and_wrong_assembly_identity_fail_explicitly(self):
        for name, dependencies in (
            ("missing", []),
            ("wrong-version", [self.folder / "wrong/Wide.dll", self.fixture / "Narrow.dll"]),
        ):
            with self.subTest(case=name):
                result, output = self.decode(name, dependencies)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("Missing metadata assembly identity: wide, Version=1.2.3.4", result.stderr)
                self.assertFalse(output.exists())

    def test_duplicate_assembly_identity_is_ambiguous(self):
        duplicate = self.folder / "duplicate.dll"
        shutil.copyfile(self.fixture / "Wide.dll", duplicate)
        result, output = self.decode("ambiguous", [self.fixture / "Wide.dll", duplicate])
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Ambiguous metadata assembly identity: wide, Version=1.2.3.4", result.stderr)
        self.assertFalse(output.exists())


if __name__ == "__main__":
    unittest.main()
