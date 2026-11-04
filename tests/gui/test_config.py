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
    assert profile.preferences() == (name, 7, "system")
    sheet = app.settings()
    general = app.settings_page(sheet, 0)
    assert n.send(n.get_item(general, 1300), win32con.CB_GETCURSEL) == index
    app.close_settings(sheet, accept=False)
    app.close()
    restored = launch()
    if label is not None:
        assert restored.toolbar_text(1040) == label


@pytest.mark.parametrize("theme,index,categories", [
    ("system", 0, 0), ("light", 1, 7), ("dark", 2, 127),
])
def test_preferences_survive_restart(launch, profile, theme, index, categories):
    profile.set("Language", winreg.REG_SZ, "zh-CN")
    profile.set("SearchCategories", winreg.REG_DWORD, categories)
    profile.set("Theme", winreg.REG_SZ, theme)
    app = launch()
    sheet = app.settings()
    general = app.settings_page(sheet, 0)
    n.send(n.get_item(general, 1300), win32con.CB_SETCURSEL, 1)
    app.close_settings(sheet)
    expected = ("en-US", categories, theme)
    with winreg.OpenKey(winreg.HKEY_CURRENT_USER, profile.root) as key:
        for name, value, kind in [("Language", "en-US", winreg.REG_SZ),
                                  ("SearchCategories", categories, winreg.REG_DWORD),
                                  ("Theme", theme, winreg.REG_SZ)]:
            assert winreg.QueryValueEx(key, name) == (value, kind)
    app.close()
    restored = launch()
    sheet = restored.settings()
    general = restored.settings_page(sheet, 0)
    assert n.send(n.get_item(general, 1300), win32con.CB_GETCURSEL) == 1
    control = n.get_item(general, 1301)
    assert n.send(control, win32con.CB_GETCURSEL) == (index if win32gui.IsWindowEnabled(control) else 0)
    search = restored.settings_page(sheet, 1)
    assert sum(n.send(n.get_item(search, 1310 + bit), win32con.BM_GETCHECK) << bit
               for bit in range(7)) == categories
    restored.close_settings(sheet)
    assert profile.preferences() == expected


@pytest.mark.parametrize("name,kind,value,expected", [
    ("Language", None, None, (0, 15, 2)),
    ("Language", winreg.REG_SZ, "other", (0, 15, 2)),
    ("Language", winreg.REG_DWORD, 1, (0, 15, 2)),
    ("SearchCategories", None, None, (2, 7, 2)),
    ("SearchCategories", winreg.REG_DWORD, 128, (2, 7, 2)),
    ("SearchCategories", winreg.REG_SZ, "15", (2, 7, 2)),
    ("Theme", None, None, (2, 15, 0)),
    ("Theme", winreg.REG_SZ, "other", (2, 15, 0)),
    ("Theme", winreg.REG_DWORD, 2, (2, 15, 0)),
])
def test_invalid_preferences_fall_back_independently(launch, profile, name, kind, value, expected):
    profile.set("Language", winreg.REG_SZ, "zh-CN")
    profile.set("SearchCategories", winreg.REG_DWORD, 15)
    profile.set("Theme", winreg.REG_SZ, "dark")
    if kind is None:
        with winreg.OpenKey(winreg.HKEY_CURRENT_USER, profile.root, 0, winreg.KEY_SET_VALUE) as key:
            winreg.DeleteValue(key, name)
    else:
        profile.set(name, kind, value)
    original = profile.preferences()
    app = launch()
    sheet = app.settings()
    general = app.settings_page(sheet, 0)
    assert n.send(n.get_item(general, 1300), win32con.CB_GETCURSEL) == expected[0]
    control = n.get_item(general, 1301)
    assert n.send(control, win32con.CB_GETCURSEL) == (expected[2] if win32gui.IsWindowEnabled(control) else 0)
    search = app.settings_page(sheet, 1)
    assert sum(n.send(n.get_item(search, 1310 + bit), win32con.BM_GETCHECK) << bit
               for bit in range(7)) == expected[1]
    app.close_settings(sheet, accept=False)
    assert profile.preferences() == original
