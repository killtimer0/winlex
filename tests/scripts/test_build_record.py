"""Source, PE and archive contracts independent of installed target binaries."""
import hashlib
import importlib.util
import json
from pathlib import Path
import struct
import subprocess
import tempfile
import unittest
import zipfile


ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location("build_record", ROOT / "scripts/build-record.py")
record = importlib.util.module_from_spec(spec)
spec.loader.exec_module(record)
POLICIES = json.loads((ROOT / "inputs/targets.json").read_text())


def pe_image(policy, dlls, kind=3, delay=False):
    data = bytearray(2048)
    data[:2] = b"MZ"
    struct.pack_into("<I", data, 0x3c, 0x80)
    data[0x80:0x84] = b"PE\0\0"
    struct.pack_into("<HH", data, 0x84, policy["machine"], 1)
    optional = 0x98
    is64 = policy["machine"] != 332
    header_size = 240 if is64 else 224
    struct.pack_into("<H", data, 0x94, header_size)
    struct.pack_into("<H", data, optional, 0x20b if is64 else 0x10b)
    struct.pack_into("<HH", data, optional + 48, *map(int, policy["subsystem"].split(".")))
    struct.pack_into("<I", data, optional + 60, 0x200)
    struct.pack_into("<H", data, optional + 68, kind)
    struct.pack_into("<IIII", data, optional + header_size + 8, 0x600, 0x1000, 0x600, 0x200)
    directory, stride = (13, 32) if delay else (1, 20)
    directories = optional + (112 if is64 else 96)
    struct.pack_into("<II", data, directories + directory * 8, 0x1000, stride * (len(dlls) + 1))
    for index, dll in enumerate(dlls):
        if delay:
            struct.pack_into("<II", data, 0x200 + index * stride, 1, 0x1100 + index * 64)
        else:
            struct.pack_into("<I", data, 0x200 + index * stride + 12, 0x1100 + index * 64)
        name = dll.encode("ascii") + b"\0"
        data[0x300 + index * 64:0x300 + index * 64 + len(name)] = name
    return bytes(data)


class BuildRecordContracts(unittest.TestCase):
    def test_pe_contract_for_all_targets_and_delay_imports(self):
        for arch, policy in POLICIES.items():
            for delay in (False, True):
                for kind in (2, 3):
                    with self.subTest(arch=arch, delay=delay, kind=kind):
                        imports = record.verify_pe(pe_image(policy, [policy["crt"], "KERNEL32.dll"], kind, delay), policy, kind)
                        self.assertIn(policy["crt"], imports)

    def test_pe_rejects_wrong_machine_subsystem_and_runtime(self):
        policy = POLICIES["arm64"]
        for field, value, error in (("machine", 332, "machine mismatch"),
                                    ("subsystem", "6.00", "subsystem version mismatch")):
            with self.subTest(field=field), self.assertRaisesRegex(ValueError, error):
                record.verify_pe(pe_image({**policy, field: value}, [policy["crt"]]), policy, 3)
        with self.assertRaisesRegex(ValueError, "subsystem kind mismatch"):
            record.verify_pe(pe_image(policy, [policy["crt"]], kind=2), policy, 3)
        with self.assertRaisesRegex(ValueError, "Required system CRT"):
            record.verify_pe(pe_image(policy, ["kernel32.dll"]), policy, 3)
        for dll in ("vcruntime140.dll", "msvcp140.dll", "ucrtbased.dll", "msvcrt.dll"):
            for delay in (False, True):
                with self.subTest(dll=dll, delay=delay), self.assertRaisesRegex(ValueError, "Unexpected CRT"):
                    record.verify_pe(pe_image(policy, [policy["crt"], dll], delay=delay), policy, 3)

    def test_source_identity_tracks_content_but_not_ignored_outputs(self):
        with tempfile.TemporaryDirectory(dir=ROOT / "tmp") as folder:
            root = Path(folder)
            subprocess.run(["git", "init", "-q", str(root)], check=True)
            (root / ".gitignore").write_text("/artifacts/\n", encoding="utf-8")
            (root / "source.rs").write_text("original", encoding="utf-8")
            subprocess.run(["git", "add", "."], cwd=root, check=True)
            subprocess.run(["git", "-c", "user.name=Contract", "-c", "user.email=contract@example.invalid",
                            "commit", "-qm", "fixture"], cwd=root, check=True)
            before = record.snapshot(root)["source"]
            self.assertFalse(before["dirty"])
            (root / "artifacts").mkdir()
            (root / "artifacts/program.exe").write_bytes(b"output")
            self.assertEqual(record.snapshot(root)["source"], before)
            (root / "source.rs").write_text("changed", encoding="utf-8")
            changed = record.snapshot(root)["source"]
            self.assertTrue(changed["dirty"])
            self.assertNotEqual(changed["sha256"], before["sha256"])
            (root / "new.rs").write_text("untracked source", encoding="utf-8")
            self.assertNotEqual(record.snapshot(root)["source"]["sha256"], changed["sha256"])

    def test_archive_rejects_tampered_members_and_runtime_dlls(self):
        with tempfile.TemporaryDirectory(dir=ROOT / "tmp") as folder:
            path = Path(folder) / "archive.zip"
            binary = b"executable"
            digest = hashlib.sha256(binary).hexdigest()
            records = [{"executable": "winlex.exe", "executable_sha256": digest}]
            for extra, checksum, error in ((False, digest, None), (False, "0" * 64, "hash mismatch"),
                                           (True, digest, "Unexpected runtime")):
                with zipfile.ZipFile(path, "w") as archive:
                    archive.writestr("winlex.exe", binary)
                    archive.writestr("SHA256SUMS.txt", f"{checksum}  winlex.exe\n")
                    if extra:
                        archive.writestr("runtime.DLL", b"runtime")
                if error:
                    with self.assertRaisesRegex(ValueError, error):
                        record.verify_archive(path, records)
                else:
                    record.verify_archive(path, records)


if __name__ == "__main__":
    unittest.main()
