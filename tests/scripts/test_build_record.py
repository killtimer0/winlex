"""Source, PE and archive contracts independent of installed target binaries."""
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import struct
import subprocess
import tempfile
import unittest
import warnings
import zipfile
from unittest.mock import patch


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
    def test_archive_member_names_must_be_unique_on_windows(self):
        with tempfile.TemporaryDirectory(dir=ROOT / "tmp") as folder:
            path = Path(folder) / "members.zip"
            for names, accepted in [(("a.txt", "b.txt"), True),
                                    (("same.txt", "same.txt"), False),
                                    (("Readme.txt", "README.TXT"), False)]:
                with self.subTest(names=names):
                    payload = b"material"
                    digest = hashlib.sha256(payload).hexdigest()
                    with warnings.catch_warnings():
                        warnings.filterwarnings("ignore", message="Duplicate name:", category=UserWarning)
                        with zipfile.ZipFile(path, "w") as archive:
                            for name in names:
                                archive.writestr(name, payload)
                            archive.writestr("SHA256SUMS.txt", "".join(
                                f"{digest}  {name}\n" for name in dict.fromkeys(names)))
                    if accepted:
                        self.assertEqual(record.verify_contents(path), dict.fromkeys(names, payload))
                    else:
                        with self.assertRaises(ValueError):
                            record.verify_contents(path)

    def test_archive_literal_paths_old_timestamps_and_existing_output(self):
        with tempfile.TemporaryDirectory(prefix="打包 [contract] ", dir=ROOT / "tmp") as folder:
            directory = Path(folder)
            stage = directory / "来源 [files]"
            stage.mkdir()
            license = stage / "许可.txt"
            license.write_bytes(b"license")
            os.utime(license, (1, 1))
            archive = directory / "成品 [archive].zip"
            record.create_archive(stage, archive)
            with zipfile.ZipFile(archive) as package:
                self.assertEqual(package.read("许可.txt"), b"license")
                self.assertEqual(package.getinfo("许可.txt").date_time[:3], (1980, 1, 1))
            before = archive.read_bytes()
            with self.assertRaises(FileExistsError):
                record.create_archive(stage, archive)
            self.assertEqual(archive.read_bytes(), before)

    def test_pe_contract_for_all_targets_and_delay_imports(self):
        for arch, policy in POLICIES.items():
            for delay in (False, True):
                for kind in (2, 3):
                    with self.subTest(arch=arch, delay=delay, kind=kind):
                        record.verify_pe(pe_image(policy, [policy["crt"], "KERNEL32.dll"], kind, delay), policy, kind)

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

    def test_source_records_commit_and_dirty_state(self):
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
            self.assertEqual(changed["commit"], before["commit"])

    def test_source_archive_without_git_can_record_tool_versions(self):
        def command(*args, **kwargs):
            if args[0] == "git":
                raise FileNotFoundError()
            return {"rustc": "rust version", "cargo": "cargo version"}[args[0]]

        with tempfile.TemporaryDirectory(dir=ROOT / "tmp") as folder:
            with patch.object(record, "command", side_effect=command):
                snapshot = record.snapshot(Path(folder))
                self.assertIsNone(snapshot["source"])
                self.assertEqual(snapshot["rustc"], "rust version")
                self.assertEqual(snapshot["cargo"], "cargo version")

    def test_records_accept_matching_policy_and_reject_conflicts(self):
        with tempfile.TemporaryDirectory(dir=ROOT / "tmp") as folder:
            directory = Path(folder)
            policy = POLICIES["arm64"]
            paths = []
            for name, kind in (("winlex.exe", 3), ("winlex-gui.exe", 2)):
                executable = directory / name
                data = pe_image(policy, [policy["crt"]], kind)
                executable.write_bytes(data)
                path = directory / (name + ".build.json")
                path.write_text(json.dumps({
                    "schema_version": 2, "policy": policy, "configuration": "Debug",
                    "executable": str(executable),
                    "executable_sha256": hashlib.sha256(data).hexdigest(),
                }), encoding="utf-8")
                paths.append(path)
            validated = record.verify_records(paths, "arm64", "Debug")
            self.assertEqual([item["executable"] for item in validated],
                             [str(directory / name) for name in ("winlex.exe", "winlex-gui.exe")])
            bad = {**validated[1], "policy": {**policy, "minimum_windows": "10.0.19041.0"}}
            paths[1].write_text(json.dumps(bad), encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "policy mismatch"):
                record.verify_records(paths, "arm64", "Debug")

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
