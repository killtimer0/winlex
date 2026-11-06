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
            with patch.object(prepare.urllib.request, "urlopen", return_value=io.BytesIO(b"archive")):
                self.assertEqual(prepare.download("https://example.invalid", path, sha(b"archive")), path)
                self.assertEqual(prepare.download("https://example.invalid", path, sha(b"archive")), path)
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
            self.assertEqual(set(path.parent.iterdir()), {path})
            prepare.install(path, b"expected", sha(b"expected"))
            self.assertEqual(path.read_bytes(), b"expected")

    def test_default_prepares_only_build_dependencies(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            inputs = root / "inputs"
            inputs.mkdir()
            thunks = {"version": "1", "url": "thunks", "archive_sha256": "thunks-hash",
                      "objects": {"x86": sha(b"object")}}
            ltl = {"version": "2", "url": "ltl", "archive_sha256": "ltl-hash",
                   "libraries": {"platform": [{"name": "lib.lib", "sha256": sha(b"library")}]}}
            (inputs / "yy-thunks-source.json").write_text(json.dumps(thunks), encoding="utf-8")
            (inputs / "vc-ltl-source.json").write_text(json.dumps(ltl), encoding="utf-8")
            (inputs / "targets.json").write_text(json.dumps({"x86": {
                "thunks_arch": "x86", "vc_ltl_platform": "platform"}}), encoding="utf-8")
            deps = root / "deps"
            library = deps / "vc-ltl-2/sdk/TargetPlatform/platform/lib.lib"
            library.parent.mkdir(parents=True)
            library.write_bytes(b"library")
            archive = MagicMock()
            archive.__enter__.return_value.namelist.return_value = ["objects/x86/YY_Thunks_for_WinXP.obj"]
            archive.__enter__.return_value.read.return_value = b"object"
            with patch.object(prepare, "ROOT", root), patch.object(prepare, "DEPS", deps), \
                 patch("sys.argv", [str(SCRIPT)]), \
                 patch.object(prepare, "download", side_effect=lambda url, path, digest: path) as download, \
                 patch.object(prepare.zipfile, "ZipFile", return_value=archive), \
                 patch("py7zr.SevenZipFile"):
                prepare.main()
                self.assertEqual({(call.args[0], call.args[2]) for call in download.call_args_list},
                                 {(item["url"], item["archive_sha256"]) for item in (thunks, ltl)})
                self.assertEqual((deps / "yy-thunks-1/x86/YY_Thunks_for_WinXP.obj").read_bytes(), b"object")
                library.write_bytes(b"corrupt")
                with self.assertRaisesRegex(ValueError, "SHA256 mismatch"):
                    prepare.main()

    def test_arm64_preparation_does_not_fetch_thunks(self):
        ltl = prepare.manifest("vc-ltl-source.json")
        thunks = prepare.manifest("yy-thunks-source.json")
        with patch("sys.argv", [str(SCRIPT), "--architecture", "arm64"]), \
             patch.object(prepare, "download", side_effect=lambda url, path, digest: path) as download, \
             patch("py7zr.SevenZipFile"), patch.object(prepare, "check"):
            prepare.main()
            urls = {call.args[0] for call in download.call_args_list}
            self.assertIn(ltl["url"], urls)
            self.assertNotIn(thunks["url"], urls)

    def test_verify_local_inputs_checks_required_libraries_and_allows_extra_files(self):
        with tempfile.TemporaryDirectory(dir=SCRIPT.parents[1] / "tmp") as folder:
            sdk = Path(folder)
            directory = sdk / "TargetPlatform/platform"
            directory.mkdir(parents=True)
            library = directory / "lib.lib"
            library.write_bytes(b"library")
            manifests = {
                "targets.json": {"arm64": {"vc_ltl_platform": "platform", "thunks_arch": None}},
                "vc-ltl-source.json": {"libraries": {"platform": [{"name": "lib.lib", "sha256": sha(b"library")}]}}
            }
            with patch.object(prepare, "manifest", side_effect=manifests.__getitem__):
                self.assertEqual(prepare.verify_inputs("arm64", sdk)["vc_ltl_libraries"],
                                 manifests["vc-ltl-source.json"]["libraries"]["platform"])
                library.write_bytes(b"corrupt")
                with self.assertRaisesRegex(ValueError, "SHA256 mismatch"):
                    prepare.verify_inputs("arm64", sdk)
                library.unlink()
                with self.assertRaises(FileNotFoundError):
                    prepare.verify_inputs("arm64", sdk)
                library.write_bytes(b"library")
                (directory / "extra.lib").touch()
                prepare.verify_inputs("arm64", sdk)


if __name__ == "__main__":
    unittest.main()
