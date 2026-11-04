"""Contracts for preparing pinned build dependencies without WinMD inputs."""
import hashlib
import importlib.util
import io
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import MagicMock, patch


SCRIPT = Path(__file__).resolve().parents[2] / "scripts" / "prepare-inputs.py"
spec = importlib.util.spec_from_file_location("prepare_inputs", SCRIPT)
prepare = importlib.util.module_from_spec(spec)
spec.loader.exec_module(prepare)


def sha(data):
    return hashlib.sha256(data).hexdigest()


class PrepareInputsContracts(unittest.TestCase):
    def test_download_checks_new_and_cached_archives(self):
        with tempfile.TemporaryDirectory() as folder:
            path = Path(folder) / "archive.zip"
            with patch.object(prepare.urllib.request, "urlopen", return_value=io.BytesIO(b"archive")) as urlopen:
                self.assertEqual(prepare.download("https://example.invalid", path, sha(b"archive")), path)
                self.assertEqual(prepare.download("https://example.invalid", path, sha(b"archive")), path)
                urlopen.assert_called_once()
                path.write_bytes(b"corrupt")
                with self.assertRaisesRegex(ValueError, "SHA256 mismatch"):
                    prepare.download("https://example.invalid", path, sha(b"archive"))
            path.unlink()
            with patch.object(prepare.urllib.request, "urlopen", return_value=io.BytesIO(b"corrupt")):
                with self.assertRaisesRegex(ValueError, "SHA256 mismatch"):
                    prepare.download("https://example.invalid", path, sha(b"archive"))
                self.assertFalse(path.exists())

    def test_install_rejects_bad_hash_without_replacing_existing_file(self):
        with tempfile.TemporaryDirectory() as folder:
            path = Path(folder) / "object.obj"
            path.write_bytes(b"existing")
            with self.assertRaisesRegex(ValueError, "SHA256 mismatch"):
                prepare.install(path, b"corrupt", sha(b"expected"))
            self.assertEqual(path.read_bytes(), b"existing")
            self.assertFalse(path.with_suffix(".obj.partial").exists())
            prepare.install(path, b"expected", sha(b"expected"))
            self.assertEqual(path.read_bytes(), b"expected")

    def test_default_and_compatibility_alias_prepare_only_build_dependencies(self):
        for args in ([], ["--build-deps"]):
            with self.subTest(args=args), tempfile.TemporaryDirectory() as folder:
                root = Path(folder)
                inputs = root / "inputs"
                inputs.mkdir()
                thunks = {"version": "1", "url": "thunks", "archive_sha256": "thunks-hash",
                          "objects": {"x86": sha(b"object")}}
                ltl = {"version": "2", "url": "ltl", "archive_sha256": "ltl-hash",
                       "libraries": {"platform": [{"name": "lib.lib", "sha256": sha(b"library")}]}}
                (inputs / "yy-thunks-source.json").write_text(json.dumps(thunks), encoding="utf-8")
                (inputs / "vc-ltl-source.json").write_text(json.dumps(ltl), encoding="utf-8")
                deps = root / "deps"
                library = deps / "vc-ltl-2/sdk/TargetPlatform/platform/lib.lib"
                library.parent.mkdir(parents=True)
                library.write_bytes(b"library")
                archive = MagicMock()
                archive.__enter__.return_value.namelist.return_value = ["objects/x86/YY_Thunks_for_WinXP.obj"]
                archive.__enter__.return_value.read.return_value = b"object"
                with patch.object(prepare, "ROOT", root), patch.object(prepare, "DEPS", deps), \
                     patch("sys.argv", [str(SCRIPT), *args]), \
                     patch.object(prepare, "download", side_effect=lambda url, path, digest: path) as download, \
                     patch.object(prepare.zipfile, "ZipFile", return_value=archive), \
                     patch.object(prepare.shutil, "which", return_value="7z"), \
                     patch.object(prepare.subprocess, "run") as run:
                    prepare.main()
                    self.assertEqual([call.args[0] for call in download.call_args_list], ["thunks", "ltl"])
                    self.assertEqual([call.args[2] for call in download.call_args_list], ["thunks-hash", "ltl-hash"])
                    self.assertEqual((deps / "yy-thunks-1/x86/YY_Thunks_for_WinXP.obj").read_bytes(), b"object")
                    run.assert_called_once()
                    library.write_bytes(b"corrupt")
                    with self.assertRaisesRegex(ValueError, "SHA256 mismatch"):
                        prepare.main()


if __name__ == "__main__":
    unittest.main()
