"""Exercise build-script architecture policies without cross-linking executables."""
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[2]


class BuildPolicyContracts(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.temp = tempfile.TemporaryDirectory()
        cls.root = Path(cls.temp.name)
        cls.build = cls.root / "build-script.exe"
        cls.rc = cls.root / "rc.exe"
        subprocess.run(["rustc", "--edition=2024", str(ROOT / "build.rs"), "-o", str(cls.build)], check=True)
        source = cls.root / "rc.rs"
        source.write_text("fn main() {}", encoding="utf-8")
        subprocess.run(["rustc", str(source), "-o", str(cls.rc)], check=True)

    @classmethod
    def tearDownClass(cls):
        cls.temp.cleanup()

    def run_build(self, target, *, machine=None, platform=None, static=True):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            env = os.environ.copy()
            env.update(TARGET=target, CARGO_CFG_TARGET_FEATURE="crt-static" if static else "",
                       OUT_DIR=str(root), CARGO_MANIFEST_DIR=str(ROOT), CARGO_PKG_VERSION="0.1.0",
                       WINLEX_RC_EXE=str(self.rc))
            env.pop("WINLEX_YY_THUNKS_OBJ", None)
            env.pop("WINLEX_VC_LTL_ROOT", None)
            if target == "aarch64-pc-windows-msvc":
                env["WINLEX_YY_THUNKS_OBJ"] = str(root / "absent.obj")
            if platform:
                libdir = root / "TargetPlatform" / platform
                libdir.mkdir(parents=True)
                for name in ("libucrt.lib", "libvcruntime.lib"):
                    (libdir / name).touch()
                env["WINLEX_VC_LTL_ROOT"] = str(root)
                if machine is not None:
                    obj = root / "thunks.obj"
                    obj.write_bytes(machine.to_bytes(2, "little") + bytes(18))
                    env["WINLEX_YY_THUNKS_OBJ"] = str(obj)
            result = subprocess.run([str(self.build)], env=env, text=True, capture_output=True)
            metadata = json.loads((root / "compatibility.json").read_text()) if result.returncode == 0 else None
            return result, metadata

    def test_architecture_contract(self):
        cases = [
            ("i686-pc-windows-msvc", 0x14c, "5.01", "5.1.2600.0", "5.1.2600.0/lib/Win32"),
            ("x86_64-pc-windows-msvc", 0x8664, "5.02", "5.2.3790.0", "5.2.3790.0/lib/x64"),
            ("aarch64-pc-windows-msvc", None, "10.00", "10.0.17763.0", "10.0.10240.0/lib/ARM64"),
        ]
        for target, machine, subsystem, minimum, platform in cases:
            with self.subTest(target=target):
                result, metadata = self.run_build(target, machine=machine, platform=platform)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(metadata, {"target": target, "minimum_windows": minimum, "vc_ltl_platform": platform})
                for binary, kind in (("winlex", "CONSOLE"), ("winlex-gui", "WINDOWS")):
                    self.assertIn(f"cargo:rustc-link-arg-bin={binary}=/SUBSYSTEM:{kind},{subsystem}", result.stdout)
                    self.assertEqual(any(line.startswith(f"cargo:rustc-link-arg-bin={binary}=") and
                                         line.endswith("thunks.obj") for line in result.stdout.splitlines()),
                                      machine is not None)
                self.assertIn("cargo:rustc-link-search=native=", result.stdout)

    def test_static_crt_and_vc_ltl_are_required(self):
        for target in ("i686-pc-windows-msvc", "x86_64-pc-windows-msvc", "aarch64-pc-windows-msvc"):
            with self.subTest(target=target):
                result, _ = self.run_build(target, static=False)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("+crt-static", result.stderr)
        for target in ("i686-pc-windows-msvc", "x86_64-pc-windows-msvc", "aarch64-pc-windows-msvc"):
            result, _ = self.run_build(target)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("WINLEX_VC_LTL_ROOT", result.stderr)

    def test_legacy_thunks_are_required(self):
        for target, platform in (("i686-pc-windows-msvc", "5.1.2600.0/lib/Win32"),
                                 ("x86_64-pc-windows-msvc", "5.2.3790.0/lib/x64")):
            with self.subTest(target=target):
                result, _ = self.run_build(target, platform=platform)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("WINLEX_YY_THUNKS_OBJ", result.stderr)

    def test_legacy_object_machine_is_checked(self):
        result, _ = self.run_build("i686-pc-windows-msvc", machine=0x8664, platform="5.1.2600.0/lib/Win32")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("COFF architecture", result.stderr)


if __name__ == "__main__":
    unittest.main()
