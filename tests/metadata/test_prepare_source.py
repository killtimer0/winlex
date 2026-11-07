import hashlib
import io
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
import zipfile

from prepare_source import prepare


class MetadataInputContracts(unittest.TestCase):
    def test_download_verifies_payload_and_reuses_verified_input(self):
        data = b"pinned metadata fixture"
        source = {"package": "Fixture.Metadata", "version": "1.0.0",
                  "file": "Fixture.winmd", "sha256": hashlib.sha256(data).hexdigest()}
        archive = io.BytesIO()
        with zipfile.ZipFile(archive, "w") as package:
            package.writestr("Windows.Win32/Fixture.winmd", data)
        with tempfile.TemporaryDirectory() as directory:
            destination = Path(directory) / "inputs" / source["file"]
            with patch("prepare_source.urllib.request.urlopen", return_value=io.BytesIO(archive.getvalue())):
                prepare(source, destination)
            self.assertEqual(destination.read_bytes(), data)
            with patch("prepare_source.urllib.request.urlopen", side_effect=AssertionError("unexpected download")):
                prepare(source, destination)

    def test_invalid_package_never_replaces_existing_input(self):
        source = {"package": "Fixture.Metadata", "version": "1.0.0",
                  "file": "Fixture.winmd", "sha256": hashlib.sha256(b"expected").hexdigest()}
        for names in [["Fixture.winmd"], [], ["a/Fixture.winmd", "b/Fixture.winmd"]]:
            with self.subTest(names=names), tempfile.TemporaryDirectory() as directory:
                archive = io.BytesIO()
                with zipfile.ZipFile(archive, "w") as package:
                    for name in names:
                        package.writestr(name, b"untrusted payload")
                destination = Path(directory) / source["file"]
                destination.write_bytes(b"existing input")
                with patch("prepare_source.urllib.request.urlopen", return_value=io.BytesIO(archive.getvalue())):
                    with self.assertRaises(ValueError):
                        prepare(source, destination)
                self.assertEqual(destination.read_bytes(), b"existing input")
                self.assertFalse(destination.with_suffix(".winmd.partial").exists())


if __name__ == "__main__":
    unittest.main()
