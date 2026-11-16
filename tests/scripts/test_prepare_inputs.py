"""Contracts for preparing pinned build dependencies without WinMD inputs."""
import hashlib
import importlib.util
import io
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
import py7zr


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

    def test_prepares_selected_dependencies_from_verified_archives(self):
        thunks_zip, ltl_7z = io.BytesIO(), io.BytesIO()
        with prepare.zipfile.ZipFile(thunks_zip, 'w') as package:
            package.writestr('objects/x86/YY_Thunks_for_WinXP.obj', b'object')
        with py7zr.SevenZipFile(ltl_7z, 'w') as package:
            for platform in ('x86', 'arm64'):
                package.writestr(platform.encode(), 'TargetPlatform/' + platform + '/lib.lib')
        thunks = dict(version='1', url='https://example.invalid/thunks',
                      archive_sha256=sha(thunks_zip.getvalue()), objects={'x86': sha(b'object')})
        ltl = dict(version='2', url='https://example.invalid/ltl',
                   archive_sha256=sha(ltl_7z.getvalue()), libraries={
                       arch: [dict(name='lib.lib', sha256=sha(arch.encode()))] for arch in ('x86', 'arm64')})
        for architecture in (None, 'arm64'):
            with self.subTest(architecture=architecture), tempfile.TemporaryDirectory() as folder:
                root = Path(folder)
                inputs, deps = root / 'inputs', root / 'deps'
                inputs.mkdir()
                manifests = {'yy-thunks-source.json': thunks, 'vc-ltl-source.json': ltl,
                             'targets.json': {arch: dict(vc_ltl_platform=arch,
                                                        thunks_arch='x86' if arch == 'x86' else None)
                                              for arch in ('x86', 'arm64')}}
                for name, value in manifests.items():
                    (inputs / name).write_text(json.dumps(value), encoding='utf-8')
                payloads = {ltl['url']: ltl_7z.getvalue()}
                if architecture is None:
                    payloads[thunks['url']] = thunks_zip.getvalue()
                def fetch(url, **kwargs):
                    return io.BytesIO(payloads[url])
                options = ['--architecture', architecture] if architecture else []
                with patch.object(prepare, 'ROOT', root), patch.object(prepare, 'DEPS', deps), \
                     patch('sys.argv', [str(SCRIPT), *options]), \
                     patch.object(prepare.urllib.request, 'urlopen', side_effect=fetch):
                    prepare.main()
                    for arch in (('arm64',) if architecture else ('x86', 'arm64')):
                        library = deps / 'vc-ltl-2/sdk/TargetPlatform' / arch / 'lib.lib'
                        self.assertEqual(library.read_bytes(), arch.encode())
                    obj = deps / 'yy-thunks-1/x86/YY_Thunks_for_WinXP.obj'
                    if architecture:
                        self.assertFalse((deps / 'yy-thunks-1').exists())
                    else:
                        self.assertEqual(obj.read_bytes(), b'object')
                    self.assertFalse(list(inputs.glob('*.winmd')))
                    (deps / 'vc-ltl-2/sdk.7z').write_bytes(b'corrupt')
                    with self.assertRaisesRegex(ValueError, 'SHA256 mismatch'):
                        prepare.main()

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
