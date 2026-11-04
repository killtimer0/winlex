import win32con
import win32gui
import winreg

from support import native as n


def owned_dialogs(app):
    owner = n.api(n.user, "GetWindow", n.w.HWND, n.w.HWND, n.w.UINT)
    return [hwnd for hwnd in n.windows()
            if n.class_name(hwnd) == "#32770" and n.is_visible(hwnd)
            and owner(hwnd, win32con.GW_OWNER) == app.frame]


def assert_no_dialog(app):
    n.send(app.frame, win32con.WM_NULL)
    assert win32gui.IsWindowEnabled(app.frame)
    assert not owned_dialogs(app)


def test_empty_startup_prompt_dismissal_and_runtime_removal(launch, profile, settings, ui_language):
    app = launch(metadata_prompt=True)
    prompt = n.wait(lambda: next(iter(owned_dialogs(app)), None), "metadata guide prompt")
    try:
        text = [n.window_text(hwnd) for hwnd in n.windows(prompt) if n.class_name(hwnd) == "Static"]
        expected = {
            "en-US": "No metadata sources are configured. Open the guide to getting metadata?",
            "zh-CN": "尚未配置元数据源。是否打开获取元数据的指南？",
        }
        assert expected[ui_language.name] in text
        assert not win32gui.IsWindowEnabled(app.frame)
    finally:
        n.post(prompt, win32con.WM_COMMAND, win32con.IDNO, 0)
        n.wait(lambda: not n.is_window(prompt), "metadata prompt dismissed")

    assert_no_dialog(app)
    app.pick([settings[1]])
    n.wait(lambda: app.count(app.sources) == 1, "source loaded", 45)
    app.command(1021)
    assert n.window_text(n.get_item(app.frame, 1023)) == {
        "en-US": "Get metadata", "zh-CN": "获取元数据",
    }[ui_language.name]
    app.select(0, app.sources)
    app.command(1022)
    n.wait(lambda: app.count(app.sources) == 0 and profile.get("AutoLoad") == [],
           "source removed", 45)
    assert_no_dialog(app)
    app.command(1040)
    assert_no_dialog(app)


def test_failed_configured_source_does_not_prompt(launch, profile, settings):
    missing = str(settings[2] / "missing-guide-restore.winmd")
    profile.set("AutoLoad", winreg.REG_MULTI_SZ, [missing])
    app = launch()
    app.command(1021)
    n.wait(lambda: "failed" in app.status_text() or "失败" in app.status_text(),
           "configured source load failure", 45)
    assert app.count(app.sources) == 0
    assert profile.get("AutoLoad") == [missing]
    assert_no_dialog(app)


def test_command_line_source_does_not_prompt(launch, settings):
    app = launch([settings[1]])
    n.wait(lambda: app.count(app.sources) == 1, "command-line source loaded", 45)
    assert_no_dialog(app)
