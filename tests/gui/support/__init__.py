"""Shared UI Automation initialization for legacy Windows."""
import ctypes
import importlib.util
from pathlib import Path
import sys

# Before UIA initializes, keep legacy geometry and real input in physical pixels.
# Modern Windows uses the per-thread contexts in native.py and conftest.py.
_user = ctypes.WinDLL('user32', use_last_error=True)
if not hasattr(_user, 'SetThreadDpiAwarenessContext') and hasattr(_user, 'SetProcessDPIAware'):
    _user.IsProcessDPIAware.restype = ctypes.c_int
    _user.SetProcessDPIAware.restype = ctypes.c_int
    if not _user.IsProcessDPIAware():
        assert _user.SetProcessDPIAware(), 'Cannot enable test process DPI awareness'

if sys.version_info < (3, 9):
    spec = importlib.util.find_spec("pywin32_system32")
    if spec is None or not spec.submodule_search_locations:
        raise ImportError("Install tests/gui/requirements.lock in the test interpreter")
    directory = Path(next(iter(spec.submodule_search_locations)))
    _dlls = [ctypes.WinDLL(str(directory / (name + "%d%d.dll" % sys.version_info[:2])))
             for name in ("pywintypes", "pythoncom")]
