"""Workspace version and release tag contracts."""
import importlib.util
from pathlib import Path
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "scripts" / "version.py"
spec = importlib.util.spec_from_file_location("version", SCRIPT)
version = importlib.util.module_from_spec(spec)
spec.loader.exec_module(version)


class VersionContracts(unittest.TestCase):
    def manifest(self, text):
        temporary = tempfile.TemporaryDirectory(dir=ROOT / "tmp")
        self.addCleanup(temporary.cleanup)
        root = Path(temporary.name)
        (root / "Cargo.toml").write_text(text, encoding="utf-8")
        return root

    def test_reads_workspace_version_and_validates_tag(self):
        root = self.manifest('[workspace.package]\nversion = "1.2.3"\n\n[dependencies]\n')
        self.assertEqual(version.package_version(root), "1.2.3")
        version.check_tag("v1.2.3", root)
        version.check_tag("v1.2.3-rc.1", root)
        for tag in ("v1.2.3-", "v1.2.4", "v1.2.4-rc.1"):
            with self.assertRaisesRegex(ValueError, "does not match package version"):
                version.check_tag(tag, root)

    def test_rejects_missing_workspace_version(self):
        root = self.manifest('[package]\nversion = "1.2.3"\n')
        with self.assertRaisesRegex(ValueError, "workspace package version"):
            version.package_version(root)
