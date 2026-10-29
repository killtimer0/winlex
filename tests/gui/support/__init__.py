"""Shared native test support; message tests do not claim actual-input coverage."""
import ctypes
import importlib.util
from pathlib import Path
import sys

if sys.version_info < (3, 9):
    spec = importlib.util.find_spec("pywin32_system32")
    if spec is None or not spec.submodule_search_locations:
        raise ImportError("Install tests/gui/requirements.txt in the test interpreter")
    directory = Path(next(iter(spec.submodule_search_locations)))
    _dlls = [ctypes.WinDLL(str(directory / (name + "%d%d.dll" % sys.version_info[:2])))
             for name in ("pywintypes", "pythoncom")]
