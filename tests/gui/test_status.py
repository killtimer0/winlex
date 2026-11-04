"""Status text belongs to the selected panel or document, not the whole sidebar."""
import win32con
import pytest

from support import native as n
from test_tools import tool, body


def show_panel(app, command, control):
    target = n.get_item(app.frame, control)
    if not n.is_visible(target):
        app.command(command)
    app.focus(target)


def test_panel_statuses_and_document_close_fallback(launch, settings):
    app = launch([settings[1]])
    app.query("CreateFileW")
    search_status = app.status_text()
    assert search_status.startswith(("Results ", "共 "))
    show_panel(app, 1021, 1112)
    assert app.status_text() in ("Sources: 1", "来源：1")
    show_panel(app, 1800, 1801)
    assert not app.status_text()
    app.command(1040)
    assert app.status_text() == search_status
    doc = app.open_row(app.named_rows("CreateFileW")[0])
    assert not app.status_text(), "declaration does not inherit search status"
    show_panel(app, 1021, 1112)
    app.focus(n.get_item(doc, 1203))
    assert not app.status_text()
    n.send(doc, win32con.WM_CLOSE)
    n.wait(lambda: app.status_text() in ("Sources: 1", "来源：1"), "closed document uses visible panel")
    app.command(1021)
    assert not app.status_text(), "collapsed panel does not contribute text"


def test_search_completion_does_not_replace_other_panel_or_tool_status(launch, settings):
    app = launch([settings[1]])
    doc = tool(app)
    ready = app.status_text()
    app.command(1040)
    n.set_text(app.search, "CreateFileW")
    show_panel(app, 1021, 1112)
    n.wait(lambda: app.count() and app.named_rows("CreateFileW"), "search completed in background", 45)
    assert app.status_text() in ("Sources: 1", "来源：1")
    app.focus(n.get_item(doc, 1810))
    assert app.status_text() == ready
    app.command(1040)
    n.set_text(app.search, "ERROR_ACCESS_DENIED")
    app.focus(n.get_item(doc, 1810))
    n.wait(lambda: app.count() and app.named_rows("ERROR_ACCESS_DENIED"), "second search completed", 45)
    assert app.status_text() == ready
    n.set_text(n.get_item(doc, 1810), "5")
    app.command(1815, doc)
    show_panel(app, 1800, 1801)
    n.wait(lambda: "0x00000005" in body(doc), "tool completed while sidebar active", 30)
    assert not app.status_text(), "tool completion does not replace tools panel status"


@pytest.mark.interactive
def test_real_panel_focus_selects_its_status(launch, settings):
    from support.input_state import InputState, foreground

    with InputState() as inputs:
        app = launch([settings[1]])
        app.query("CreateFileW")
        search_status = app.status_text()
        with foreground(app, inputs):
            for command, control, expected in [
                (1021, 1112, ("Sources: 1", "来源：1")),
                (1800, 1801, ("",)),
                (1040, 1102, (search_status,)),
            ]:
                app.command(command)
                target = n.get_item(app.frame, control)
                with n.window_dpi_context(app.frame):
                    left, top, right, bottom = n.rectangle(target)
                    inputs.click(left + 8, top + 8)
                n.wait(lambda: app.focused() == target, "physical panel focus")
                assert app.status_text() in expected
