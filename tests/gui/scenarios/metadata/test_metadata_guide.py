"""The empty-catalog guide is an explicit, dismissible startup choice."""
import win32con
import win32gui

from support import native as n
from support.session import invoke, message_button
from driver.metadata import assert_no_dialog, owned_dialogs, resource_text


def test_empty_startup_prompt_dismissal_and_runtime_removal(launch, profile, settings, ui_language):
    app = launch(startup='metadata-guide')
    prompt = n.wait(lambda: next(iter(owned_dialogs(app)), None), "metadata guide prompt")
    try:
        assert not win32gui.IsWindowEnabled(app.frame)
    finally:
        invoke(n.wait(lambda: message_button(prompt, win32con.IDNO), 'metadata prompt No button'))
        n.wait(lambda: not n.is_window(prompt), "metadata prompt dismissed")

    assert_no_dialog(app)
    empty = resource_text(settings[0], 2345, ui_language.langid)
    n.wait(lambda: app.status_text() == empty, "empty metadata guidance")
    menu = win32gui.GetMenu(app.frame)
    help_menu = win32gui.GetSubMenu(menu, win32gui.GetMenuItemCount(menu) - 1)
    assert 1023 in [win32gui.GetMenuItemID(help_menu, index)
                    for index in range(win32gui.GetMenuItemCount(help_menu))]
    app.pick([settings[1]])
    app.command(1021)
    n.wait(lambda: app.count(app.sources) == 1, "source loaded", 45)
    assert app.status_text() != empty
    app.select(0, app.sources)
    app.command(1022)
    n.wait(lambda: app.count(app.sources) == 0 and profile.get("AutoLoad") == [],
           "source removed", 45)
    n.wait(lambda: app.status_text() == empty, "guidance after removing last source")
    assert_no_dialog(app)
    app.command(1040)
    assert app.status_text() == empty
    assert_no_dialog(app)
