import struct
import pytest
import winreg
import win32con
import win32gui
from support import native as n


def test_normal_placement_round_trips_registry_coordinates(launch, profile):
    app = launch()
    win32gui.SetWindowPos(app.frame, 0, 70, 80, 740, 520,
                         win32con.SWP_NOZORDER | win32con.SWP_NOACTIVATE)
    bounds = n.rectangle(app.frame)
    app.close()
    saved = profile.get("Window")
    x, y, width, height, dpi, maximized = struct.unpack('<6i', saved)
    assert (x, y, width, height) == (bounds[0], bounds[1],
                                   bounds[2] - bounds[0], bounds[3] - bounds[1])
    assert 96 <= dpi <= 960 and maximized == 0
    restored = launch()
    assert n.rectangle(restored.frame) == bounds
    restored.close()
    assert profile.get("Window") == saved


def test_placement_and_sidebar_restart(launch, profile):
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
    n.send(app.frame, win32con.WM_SYSCOMMAND, win32con.SC_MAXIMIZE)
    assert n.is_zoomed(app.frame)
    app.close()
    assert (profile.get("ActivePanel"), profile.get("PanelCollapsed")) == (1, 0)
    assert struct.unpack("<6i", profile.get("Window"))[-1] == 1
    restored = launch()
    assert n.is_visible(restored.sources) and not n.is_visible(restored.results)
    assert n.is_zoomed(restored.frame)
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


@pytest.mark.parametrize("index,name,label", [
    (0, "auto", None), (1, "en-US", "Search"), (2, "zh-CN", "搜索"),
], ids=["auto", "english", "chinese"])
def test_language_option_round_trip(launch, profile, index, name, label):
    app = launch()
    sheet = app.settings()
    general = app.settings_page(sheet, 0)
    control = n.get_item(general, 1300)
    assert app.choices(control) == (["跟随系统", "English", "简体中文"] if app.toolbar_text(1040) == "搜索"
                                   else ["System default", "English", "简体中文"])
    n.send(control, win32con.CB_SETCURSEL, index)
    app.close_settings(sheet)
    assert profile.get("Preferences") == name + ":7"
    sheet = app.settings()
    general = app.settings_page(sheet, 0)
    assert n.send(n.get_item(general, 1300), win32con.CB_GETCURSEL) == index
    app.close_settings(sheet, accept=False)
    app.close()
    restored = launch()
    if label is not None:
        assert restored.toolbar_text(1040) == label
