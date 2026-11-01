"""Isolated settings with an outer-run cleanup ledger for timeout recovery."""
import os
import uuid
import winreg
from pathlib import Path


class ConfigProfile:
    def __init__(self, language="en-US"):
        self.root = "Software\\Winlex\\Tests\\" + uuid.uuid4().hex
        ledger = os.environ.get("WINLEX_TEST_LEDGER")
        if ledger:
            Path(ledger).mkdir(parents=True, exist_ok=True)
            (Path(ledger) / (self.root.rsplit("\\", 1)[-1] + ".profile")).write_text(self.root, encoding="utf-8")
        self.set("Version", winreg.REG_DWORD, 1)
        self.set("Preferences", winreg.REG_SZ, language + ":7")

    def set(self, name, kind, value):
        with winreg.CreateKeyEx(winreg.HKEY_CURRENT_USER, self.root, 0, winreg.KEY_SET_VALUE) as key:
            winreg.SetValueEx(key, name, 0, kind, value)

    def get(self, name, default=None):
        with winreg.OpenKey(winreg.HKEY_CURRENT_USER, self.root) as key:
            try:
                return winreg.QueryValueEx(key, name)[0]
            except FileNotFoundError:
                return default

    def environment(self, env=None, dpi="pmv2"):
        return {**(os.environ if env is None else env), "WINLEX_CONFIG_ROOT": self.root, "WINLEX_DPI": dpi}

    def close(self):
        try:
            winreg.DeleteKey(winreg.HKEY_CURRENT_USER, self.root)
        except FileNotFoundError:
            pass

    def __enter__(self):
        return self

    def __exit__(self, *args):
        self.close()
