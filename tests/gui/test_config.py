import struct
import pytest
import winreg
import win32con
from support import native as n


def test_placement_and_language_restart(launch, profile):
    profile.set("Window", winreg.REG_BINARY, struct.pack("<6i", 100000, 100000, 800, 600, 96, 0))
    profile.set("SidebarWidth", winreg.REG_DWORD, 310)
    profile.set("ActivePanel", winreg.REG_DWORD, 1)
    profile.set("PanelCollapsed", winreg.REG_DWORD, 1)
    app = launch()
    assert not n.is_visible(app.sources) and not n.is_visible(app.results)
    app.command(1021)
    assert n.is_visible(app.sources) and not n.is_visible(app.results)
    rect = n.rectangle(app.frame)
    assert any(m[0] <= rect[0] < rect[2] <= m[2] and m[1] <= rect[1] < rect[3] <= m[3] for m in n.monitors())
    app.command(1032)
    n.send(app.frame, win32con.WM_SYSCOMMAND, win32con.SC_MAXIMIZE)
    assert n.is_zoomed(app.frame)
    app.close()
    assert profile.get("Language") == "zh-CN"
    assert (profile.get("ActivePanel"), profile.get("PanelCollapsed")) == (1, 0)
    assert struct.unpack("<6i", profile.get("Window"))[-1] == 1
    restored = launch()
    assert n.is_visible(restored.sources) and not n.is_visible(restored.results)
    assert n.is_zoomed(restored.frame)
    assert restored.toolbar_text(1040) == "搜索"
    n.send(restored.frame, win32con.WM_SYSCOMMAND, win32con.SC_RESTORE)
    actual = n.rectangle(restored.frame)
    assert all(abs(a-b) <= 2 for a,b in zip(actual, rect))
    restored.command(1021)
    assert not n.is_visible(restored.sources)
    restored.close()
    assert profile.get("PanelCollapsed") == 1
    collapsed = launch()
    assert not n.is_visible(collapsed.sources) and not n.is_visible(collapsed.results)
    collapsed.command(1021)
    assert n.is_visible(collapsed.sources)


@pytest.mark.parametrize("command,name,label", [
    (1030, "auto", None), (1031, "en-US", "Search"), (1032, "zh-CN", "搜索"),
], ids=["auto", "english", "chinese"])
def test_language_option_round_trip(launch, profile, command, name, label):
    app = launch()
    app.command(command)
    assert profile.get("Language") == name
    menu = n.api(n.user, "GetMenu", n.w.HMENU, n.w.HWND)(app.frame)
    get_state = n.api(n.user, "GetMenuState", n.w.UINT, n.w.HMENU, n.w.UINT, n.w.UINT)
    for candidate in (1030, 1031, 1032):
        assert bool(get_state(menu, candidate, win32con.MF_BYCOMMAND) & win32con.MF_CHECKED) == (candidate == command)
    app.command(1033)
    assert profile.get("Language") == name
    app.close()
    restored = launch()
    if label is not None:
        assert restored.toolbar_text(1040) == label
