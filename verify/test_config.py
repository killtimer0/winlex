import struct
import winreg
from support import native as n


def test_placement_and_language_restart(launch, profile):
    profile.set("Window", winreg.REG_BINARY, struct.pack("<6i", 100000, 100000, 800, 600, 96, 0))
    profile.set("SidebarWidth", winreg.REG_DWORD, 310)
    app = launch()
    rect = n.rectangle(app.frame)
    assert any(m[0] <= rect[0] < rect[2] <= m[2] and m[1] <= rect[1] < rect[3] <= m[3] for m in n.monitors())
    app.command(1032)
    n.send(app.frame, 0x112, 0xF030)
    assert n.is_zoomed(app.frame)
    app.close()
    assert profile.get("Language") == "zh-CN"
    assert struct.unpack("<6i", profile.get("Window"))[-1] == 1
    restored = launch()
    assert n.is_zoomed(restored.frame)
    assert n.window_text(n.get_item(restored.frame, 1040)) == "搜索"
    n.send(restored.frame, 0x112, 0xF120)
    actual = n.rectangle(restored.frame)
    assert all(abs(a-b) <= 2 for a,b in zip(actual, rect))
