"""Delivery integrity contracts independent of installed cross-compilers."""
import importlib
import contextlib
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
import zipfile

from test_build_record import pe_image, pdb_image

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))
delivery = importlib.import_module("delivery")


class DeliveryContracts(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="交付 [contract] ", dir=ROOT / "tmp")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.inputs = self.root / "inputs"
        self.inputs.mkdir()
        self.output = self.root / "output"
        self.policies = json.loads((ROOT / "inputs/targets.json").read_text())
        for arch, policy in self.policies.items():
            programs = {"winlex.exe": pe_image(policy, [policy["crt"]]),
                        "winlex-gui.exe": pe_image(policy, [policy["crt"]], kind=2),
                        "LICENSES.txt": b"See matching licenses.zip"}
            programs.update({name: b"retained notice" for name in delivery.NOTICES})
            self.archive(self.inputs / f"winlex-{arch}.zip", programs)
            self.program_checksum(arch)
            symbols = {"winlex.pdb": pdb_image(), "winlex_gui.pdb": pdb_image(2)}
            self.archive(self.inputs / f"winlex-{arch}-symbols.zip", symbols)
            materials = {name: b"retained notice" for name in delivery.REQUIRED_LICENSES}
            materials.update({"licenses/crates/shared/LICENSE": b"common license",
                              "licenses/rust/licenses/MIT.txt": b"Rust license",
                              f"licenses/crates/{arch}/LICENSE": arch.encode()})
            provenance = {
                "architecture": arch, "cargo_lock_sha256": "pinned", "compatibility_sources": {},
                "builds": [{"source": {"commit": "revision", "dirty": False}, "configuration": "Debug",
                            "rustc": "fixed rust", "cargo": "fixed cargo", "policy": policy,
                            "executable_sha256": delivery.digest(programs[name])}
                           for name in ("winlex.exe", "winlex-gui.exe")],
            }
            for build, name in zip(provenance["builds"], symbols):
                build.update(pdb_name=name, pdb_sha256=delivery.digest(symbols[name]))
            materials[f"builds/{arch}.json"] = json.dumps(provenance).encode()
            self.archive(self.inputs / f"winlex-{arch}-licenses.zip", materials)

    def archive(self, path, files):
        with zipfile.ZipFile(path, "w", compression=zipfile.ZIP_DEFLATED) as archive:
            for name, data in files.items():
                archive.writestr(name, data)
            archive.writestr("SHA256SUMS.txt", "".join(
                f"{delivery.digest(data)}  {name}\n" for name, data in sorted(files.items())))

    def program_checksum(self, arch):
        path = self.inputs / f"winlex-{arch}.zip"
        path.with_suffix(".zip.sha256").write_text(
            f"{delivery.digest(path.read_bytes())}  {path.name}\n", encoding="utf-8")

    def change(self, archive, edit):
        path = self.inputs / archive
        files = delivery.record.verify_contents(path)
        edit(files)
        self.archive(path, files)

    def assemble(self):
        delivery.assemble(self.inputs, self.output, "Debug", "revision")

    def assert_refused(self, reason):
        with self.assertRaisesRegex(ValueError, reason):
            self.assemble()
        self.assertFalse(self.output.exists())

    def test_union_deduplicates_and_checks_complete_delivery(self):
        result = subprocess.run([
            sys.executable, str(ROOT / "scripts/delivery.py"), "assemble",
            "--inputs", str(self.inputs), "--output", str(self.output),
            "--configuration", "Debug", "--commit", "revision",
        ], capture_output=True, text=True, encoding="utf-8", errors="replace")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual({path.name for path in self.output.iterdir()}, {
            "winlex-x86.zip", "winlex-x64.zip", "winlex-arm64.zip", "licenses.zip", "SHA256SUMS",
            "winlex-x86-symbols.zip", "winlex-x64-symbols.zip", "winlex-arm64-symbols.zip"})
        files = delivery.record.verify_contents(self.output / "licenses.zip")
        for arch in self.policies:
            self.assertEqual(files[f"licenses/crates/{arch}/LICENSE"], arch.encode())
            self.assertIn(f"builds/{arch}.json", files)
        self.assertEqual(files["licenses/crates/shared/LICENSE"], b"common license")
        for line in (self.output / "SHA256SUMS").read_text().splitlines():
            digest, name = line.split("  ", 1)
            self.assertEqual(delivery.digest((self.output / name).read_bytes()), digest)
        before = (self.output / "licenses.zip").read_bytes()
        with self.assertRaisesRegex(ValueError, "Output already exists"):
            self.assemble()
        self.assertEqual((self.output / "licenses.zip").read_bytes(), before)

    def test_missing_architecture_or_materials_refuse_delivery(self):
        path = self.inputs / "winlex-arm64.zip"
        before = path.read_bytes()
        path.unlink()
        with self.assertRaises(FileNotFoundError):
            self.assemble()
        self.assertFalse(self.output.exists())
        path.write_bytes(before)
        self.change("winlex-x64-licenses.zip", lambda files: files.pop("licenses/VC-LTL-LICENSE"))
        self.assert_refused("Missing required license")

    def test_delivery_with_cp1252_output(self):
        for invocation in ("direct", "subprocess"):
            with self.subTest(invocation=invocation):
                output = self.root / invocation
                if invocation == "direct":
                    with io.TextIOWrapper(io.BytesIO(), encoding="cp1252", errors="strict") as stream:
                        with contextlib.redirect_stdout(stream):
                            delivery.assemble(self.inputs, output, "Debug", "revision")
                        stream.flush()
                        log = stream.buffer.getvalue()
                else:
                    result = subprocess.run([
                        sys.executable, str(ROOT / "scripts/delivery.py"), "assemble",
                        "--inputs", str(self.inputs), "--output", str(output),
                        "--configuration", "Debug", "--commit", "revision",
                    ], capture_output=True,
                        env={**os.environ, "PYTHONIOENCODING": "cp1252:strict"})
                    self.assertEqual(result.returncode, 0, result.stderr)
                    log = result.stdout
                self.assertTrue(log.strip())
                log.decode("cp1252", errors="strict")
                for line in (output / "SHA256SUMS").read_text().splitlines():
                    digest, name = line.split("  ", 1)
                    self.assertEqual(delivery.digest((output / name).read_bytes()), digest)
                delivery.record.verify_contents(output / "licenses.zip")

    def test_conflicting_licenses_refuse_delivery(self):
        self.change("winlex-arm64-licenses.zip", lambda files: files.update({
            "licenses/crates/shared/LICENSE": b"different license"}))
        self.assert_refused("Conflicting license material")

    def test_missing_or_mismatched_symbols_refuse_delivery(self):
        path = self.inputs / "winlex-x86-symbols.zip"
        original = path.read_bytes()
        path.unlink()
        with self.assertRaises(FileNotFoundError):
            self.assemble()
        self.assertFalse(self.output.exists())
        path.write_bytes(original)
        self.change(path.name, lambda files: files.update({"winlex.pdb": pdb_image(2)}))
        self.assert_refused("PDB hash mismatch")

        def change_identity(files):
            provenance = json.loads(files["builds/x86.json"])
            provenance["builds"][0]["pdb_sha256"] = delivery.digest(pdb_image(2))
            files["builds/x86.json"] = json.dumps(provenance).encode()

        self.change("winlex-x86-licenses.zip", change_identity)
        self.assert_refused("Executable/PDB identity mismatch")

    def test_corrupt_program_and_license_checksums_refuse_delivery(self):
        path = self.inputs / "winlex-arm64.zip"
        original = path.read_bytes()
        path.write_bytes(original + b"corruption")
        with self.assertRaisesRegex(ValueError, "Program archive hash mismatch"):
            self.assemble()
        path.write_bytes(original)
        with zipfile.ZipFile(self.inputs / "winlex-arm64-licenses.zip", "a") as archive:
            archive.writestr("unexpected.txt", b"not checksummed")
        self.assert_refused("Archive checksum set mismatch")

    def test_wrong_source_configuration_and_executable_refuse_delivery(self):
        with self.assertRaisesRegex(ValueError, "Delivery configuration mismatch"):
            delivery.assemble(self.inputs, self.output)
        with self.assertRaisesRegex(ValueError, "Delivery source mismatch"):
            delivery.assemble(self.inputs, self.output, "Debug", "another-revision")
        self.change("winlex-arm64.zip", lambda files: files.update({"winlex.exe": b"wrong binary"}))
        self.program_checksum("arm64")
        self.assert_refused("Executable hash mismatch")

    def test_unsafe_members_and_local_paths_refuse_delivery(self):
        path = self.inputs / "winlex-arm64-licenses.zip"
        original = path.read_bytes()
        for name, data, reason in (
            ("../outside.txt", b"unsafe", "Unsafe archive member"),
            ("licenses/leak.txt", str(ROOT).encode(), "Local build path"),
            ("extra.build.json", b"record", "Unexpected runtime, metadata or build record"),
        ):
            with self.subTest(name=name):
                path.write_bytes(original)
                self.change(path.name, lambda files: files.update({name: data}))
                self.assert_refused(reason)

    @unittest.skipUnless(os.name == "nt", "Windows extended paths")
    def test_long_delivery_path(self):
        base = Path("\\\\?\\" + str(self.root)) / ("长路径-" * 35) / ("delivery-" * 18)
        delivery.assemble(self.inputs, base / "output", "Debug", "revision")
        delivery.record.verify_contents(base / "output/licenses.zip")

    @unittest.skipUnless(os.name == "nt", "Windows UNC paths")
    def test_unc_delivery_paths(self):
        share = Path("\\\\localhost\\" + ROOT.drive[0] + "$" + str(self.root)[2:])
        if not share.is_dir():
            self.skipTest("Local administrative share is unavailable")
        delivery.assemble(share / "inputs", share / "unc-output", "Debug", "revision")
        delivery.record.verify_contents(share / "unc-output/licenses.zip")


if __name__ == "__main__":
    unittest.main()
