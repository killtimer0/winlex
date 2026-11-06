"""GUI artifact retention and lossless image contracts, without a GUI session."""
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import struct
import tempfile
import unittest
from unittest.mock import patch


ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location("gui_artifacts", ROOT / "tests/gui/support/artifacts.py")
artifacts = importlib.util.module_from_spec(spec)
spec.loader.exec_module(artifacts)
spec = importlib.util.spec_from_file_location("gui_paths", ROOT / "tests/gui/support/paths.py")
paths = importlib.util.module_from_spec(spec)
spec.loader.exec_module(paths)


class GuiArtifactContracts(unittest.TestCase):
    @unittest.skipUnless(importlib.util.find_spec("uiautomation") and importlib.util.find_spec("PIL"),
                         "Requires GUI and media dependencies")
    def test_png_preserves_rgb_orientation_and_ignores_unused_bitmap_alpha(self):
        from PIL import Image
        spec = importlib.util.spec_from_file_location("gui_diagnostics", ROOT / "tests/gui/support/diagnostics.py")
        diagnostics = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(diagnostics)
        width, height = 3, 2
        top = [(255, 0, 0), (0, 255, 0), (0, 0, 255)]
        bottom = [(17, 33, 65), (0, 0, 0), (255, 255, 255)]
        pixels = bytes(channel for red, green, blue in bottom + top
                       for channel in (blue, green, red, 0))
        header = struct.pack("<IiiHHIIiiII", 40, width, height, 1, 32, 0, len(pixels), 0, 0, 0, 0)
        with tempfile.TemporaryDirectory(dir=ROOT / "tmp") as folder:
            path = Path(folder) / "像素😀.png"
            diagnostics.save_pixels(path, header, pixels)
            data = path.read_bytes()
        with Image.open(io.BytesIO(data)) as image:
            self.assertEqual(image.format, "PNG")
            self.assertEqual(image.size, (width, height))
            self.assertEqual(image.convert("RGB").tobytes(),
                             bytes(channel for rgb in top + bottom for channel in rgb))
            self.assertEqual(image.convert("RGBA").getchannel("A").getextrema(), (255, 255))

    def test_fixtures_record_hashes_before_removing_only_identical_copies(self):
        with tempfile.TemporaryDirectory(dir=ROOT / "tmp") as folder:
            output = Path(folder)
            source = output / "input.winmd"
            source.write_bytes(b"original metadata")
            digest = hashlib.sha256(source.read_bytes()).hexdigest()
            duplicate = output / "来源😀.WINMD"
            duplicate.write_bytes(source.read_bytes())
            changed = output / "corrupt.winmd"
            changed.write_bytes(b"modified metadata")
            log = output / "output.log"
            log.write_bytes(b"keep diagnostic evidence")
            artifacts.finish_fixtures(output, source, digest)
            hashes = json.loads((output / "fixture-hashes.json").read_text(encoding="utf-8"))
            self.assertEqual(hashes[duplicate.name], digest)
            self.assertEqual(hashes[changed.name], hashlib.sha256(changed.read_bytes()).hexdigest())
            self.assertFalse(duplicate.exists())
            self.assertEqual(source.read_bytes(), b"original metadata")
            self.assertEqual(changed.read_bytes(), b"modified metadata")
            self.assertEqual(log.read_bytes(), b"keep diagnostic evidence")

    def test_manifest_write_failure_does_not_remove_inputs(self):
        with tempfile.TemporaryDirectory(dir=ROOT / "tmp") as folder:
            output = Path(folder)
            source = output / "source.winmd"
            source.write_bytes(b"metadata")
            duplicate = output / "copy.winmd"
            duplicate.write_bytes(source.read_bytes())
            with patch.object(Path, "write_text", side_effect=OSError("cannot record hashes")):
                with self.assertRaisesRegex(OSError, "cannot record hashes"):
                    artifacts.finish_fixtures(output, source, hashlib.sha256(source.read_bytes()).hexdigest())
            self.assertEqual(duplicate.read_bytes(), source.read_bytes())

    def check_extended_paths(self, base):
        output = paths.extended(base / ("long-metadata-" * 12) / ("long-artifacts-" * 12))
        output.mkdir(parents=True)
        source = paths.extended(base / "input.winmd")
        source.write_bytes(b"metadata")
        duplicate = output / "来源😀.winmd"
        duplicate.write_bytes(source.read_bytes())
        artifacts.finish_fixtures(output, source, hashlib.sha256(source.read_bytes()).hexdigest())
        self.assertFalse(duplicate.exists())
        hashes = json.loads((output / "fixture-hashes.json").read_text(encoding="utf-8"))
        self.assertIn(duplicate.name, hashes)

    @unittest.skipUnless(os.name == "nt", "Windows extended paths")
    def test_long_artifact_paths(self):
        with tempfile.TemporaryDirectory(dir=ROOT / "tmp") as folder:
            self.check_extended_paths(Path(folder))

    @unittest.skipUnless(os.name == "nt", "Windows UNC paths")
    def test_unc_artifact_paths(self):
        share = Path("\\\\localhost\\" + ROOT.drive[0] + "$" + str(ROOT)[2:])
        if not share.is_dir():
            self.skipTest("Local administrative share is unavailable")
        with tempfile.TemporaryDirectory(dir=share / "tmp") as folder:
            self.check_extended_paths(Path(folder))


if __name__ == "__main__":
    unittest.main()
