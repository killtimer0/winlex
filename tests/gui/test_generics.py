"""Closed signature links retain their arguments throughout declaration navigation."""
from pathlib import Path
import ctypes as c
import json

import pytest
import win32con
import win32gui
import uiautomation as auto

from support import native as n
from test_context import find_popup, menu_key, popup, popup_window, wait_closed
from test_navigation import activate, destination, editor_selection, follow_caret, pin_active_tab, reference


@pytest.fixture
def generic_metadata(settings):
    candidates = [settings[0].parent / "ReaderFixtures.dll",
                  Path(__file__).resolve().parents[2] / "tmp/verify/reader-fixtures/ReaderFixtures.dll"]
    source = next((path for path in candidates if path.is_file()), None)
    if source is None:
        pytest.skip("ReaderFixtures.dll was not built/deployed")
    return source


def test_closed_generic_links_preserve_instances_architecture_and_history(launch, generic_metadata):
    app = launch([generic_metadata])
    app.query("Fixtures.GenericLinks.Ints")
    source = app.open_row()
    app.choose(source, "x64")
    follow_caret(app, source, reference(app, source, "IBox<int32>") + 2)
    integers = destination(app, source, "IBox<int32>", "x64")
    n.wait(lambda: "int32 Value;" in app.content(integers), "bound property", 45)
    assert "Action<int32> Changed;" in app.content(integers)
    assert "U Convert<U>" in app.content(integers)
    pin_active_tab(app)
    follow_caret(app, integers, reference(app, integers, "IBox<IBox<int32>>") + 2)
    nested = destination(app, integers, "IBox<IBox<int32>>", "x64")
    n.wait(lambda: "IBox<int32> Value;" in app.content(nested), "nested bound property", 45)
    pin_active_tab(app)
    follow_caret(app, nested, reference(app, nested, "IBox<int32> Value") + 2)
    assert destination(app, nested, "IBox<int32>", "x64") == integers
    app.choose(integers, "x86")
    n.wait(lambda: "int32 Value;" in app.content(integers), "binding survives architecture", 45)
    app.command(1055)
    assert destination(app, integers, "IBox<int32>", "x64") == integers
    app.command(1056)
    assert destination(app, integers, "IBox<int32>", "x86") == integers

    app.query("Fixtures.GenericLinks.Strings")
    strings_source = app.open_row()
    follow_caret(app, strings_source, reference(app, strings_source, "IBox<string>") + 2)
    strings = destination(app, strings_source, "IBox<string>", "x86")
    assert strings != integers
    n.wait(lambda: "string Value;" in app.content(strings), "distinct instance property", 45)
    assert "int32 Value;" in app.content(integers)
    pin_active_tab(app)

    # An instance's self reference reuses that instance, not its open definition.
    follow_caret(app, strings, reference(app, strings, "IBox<string> Same") + 2)
    assert destination(app, strings, "IBox<string>", "x86") == strings

    # The link menu can explicitly discard arguments and navigate to the definition.
    activate(app, strings)
    editor = n.get_item(strings, 1203)
    from test_navigation import editor_selection
    index = reference(app, strings, "IBox<string> Same") + 2
    editor_selection(app, editor, (index, index))
    _, items = popup(app, editor, keyboard=True)
    assert 2159 in items
    for key in (win32con.VK_DOWN, win32con.VK_DOWN, win32con.VK_RETURN):
        win32gui.PostMessage(editor, win32con.WM_KEYDOWN, key, 1)
        win32gui.PostMessage(editor, win32con.WM_KEYUP, key, -1073741823)
    wait_closed(editor)
    definition = n.wait(lambda: (lambda doc: doc if n.window_text(doc).startswith("IBox`1 - x86") else None)(
        app.active_document()), "generic definition document")
    n.wait(lambda: "T Value;" in app.content(definition), "open generic definition", 45)
    assert definition != strings

    activate(app, strings)
    button = n.get_item(strings, 1206)
    win32gui.PostMessage(strings, win32con.WM_COMMAND, 1206, button)
    _, items = find_popup(app)
    assert 2159 in items
    window = popup_window(app.process.pid)
    menu_key(window, win32con.VK_DOWN)
    menu_key(window, win32con.VK_RETURN)
    wait_closed(button)
    n.wait(lambda: app.active_document() == definition, "information menu reuses definition")


def test_winrt_closed_generic_signature_navigation(launch, settings):
    candidates = [settings[0].parent / "Windows.winmd",
                  Path(__file__).resolve().parents[2] / "inputs/Windows.winmd"]
    source = next((path for path in candidates if path.is_file()), None)
    if source is None:
        pytest.skip("Windows.winmd was not deployed")
    app = launch([source])
    app.query("Windows.Foundation.Collections.PropertySet")
    origin = app.open_row()
    app.choose(origin, "x64")
    follow_caret(app, origin, reference(app, origin, "IMap<string, object>") + 2)
    instance = destination(app, origin, "IMap<string, object>", "x64")
    n.wait(lambda: "object Lookup(" in app.content(instance), "WinRT bound lookup", 45)
    assert "string key" in app.content(instance)


def bind_dialog(app, document):
    activate(app, document)
    button = n.get_item(document, 1212)
    assert win32gui.IsWindowEnabled(button)
    assert win32gui.IsWindowVisible(button)
    win32gui.PostMessage(document, win32con.WM_COMMAND, 1212, button)
    dialog = auto.WindowControl(searchDepth=4, ClassName="#32770",
                                Compare=lambda item, depth: item.ProcessId == app.process.pid)
    n.wait(lambda: dialog.Exists(0, 0), "type binding dialog")
    return dialog


@pytest.mark.parametrize("member, argument, name", [
    ("Ints", "int32", "IBox<int32>"),
    ("Guids", "guid", "IBox<Guid>"),
    ("Nested", "Fixtures.IBox<string>", "IBox<IBox<string>>"),
    ("ArchitectureBound", "Fixtures.X64Argument", "IBox<X64Argument>"),
], ids=["scalar", "guid", "nested", "architecture-bound"])
def test_link_and_manual_binding_reuse_the_same_instance(launch, generic_metadata, member, argument, name):
    app = launch([generic_metadata])
    app.query("Fixtures.GenericLinks." + member)
    source = app.open_row()
    app.choose(source, "x64")
    follow_caret(app, source, reference(app, source, name) + 2)
    linked = destination(app, source, name, "x64")
    pin_active_tab(app)
    dialog = bind_dialog(app, linked)
    hwnd = dialog.NativeWindowHandle
    prefilled = n.window_text(n.get_item(hwnd, 1500))
    assert prefilled
    # Accepting the current value must preserve the linked instance identity.
    win32gui.PostMessage(hwnd, win32con.WM_COMMAND, 1231, n.get_item(hwnd, 1231))
    n.wait(lambda: not n.is_window(hwnd), "prefilled binding accepted")
    assert app.active_document() == linked
    dialog = bind_dialog(app, linked)
    hwnd = dialog.NativeWindowHandle
    auto.ControlFromHandle(n.get_item(hwnd, 1500)).GetValuePattern().SetValue(argument)
    win32gui.PostMessage(hwnd, win32con.WM_COMMAND, 1231, n.get_item(hwnd, 1231))
    n.wait(lambda: not n.is_window(hwnd), "manual binding accepted")
    assert app.active_document() == linked
    assert app.choice(linked) == "x64"


def test_failed_architecture_switch_keeps_instance_position_history_and_preference(launch, generic_metadata, profile):
    app = launch([generic_metadata])
    app.query("Fixtures.GenericLinks.ArchitectureBound")
    source = app.open_row()
    app.choose(source, "x64")
    follow_caret(app, source, reference(app, source, "IBox<X64Argument>") + 2)
    instance = destination(app, source, "IBox<X64Argument>", "x64")
    before = app.content(instance)
    editor = n.get_item(instance, 1203)
    wanted = editor_selection(app, editor, (10, 18))
    top = n.send(editor, win32con.EM_GETFIRSTVISIBLELINE)
    preferred = profile.get("PreferredArchitecture")
    app.choose(instance, "x86")
    assert app.choice(instance) == "x64"
    assert app.active_document() == instance
    assert app.content(instance) == before
    assert editor_selection(app, editor) == wanted
    assert n.send(editor, win32con.EM_GETFIRSTVISIBLELINE) == top
    assert profile.get("PreferredArchitecture") == preferred
    app.focus(instance)
    app.command(1055)
    n.wait(lambda: app.active_document() == source, "failure adds no history entry")
    app.command(1056)
    n.wait(lambda: app.active_document() == instance, "instance history restored")
    assert app.choice(instance) == "x64"
    assert app.content(instance) == before
    assert editor_selection(app, editor) == wanted

    # Direct binding at the rejected architecture must also fail without navigation.
    app.query("Fixtures.IBox`1")
    definition = app.open_row()
    app.choose(definition, "x86")
    dialog = bind_dialog(app, definition)
    hwnd = dialog.NativeWindowHandle
    auto.ControlFromHandle(n.get_item(hwnd, 1500)).GetValuePattern().SetValue("Fixtures.X64Argument")
    win32gui.PostMessage(hwnd, win32con.WM_COMMAND, 1231, n.get_item(hwnd, 1231))
    n.wait(lambda: bool(n.window_text(n.get_item(hwnd, 1232))), "architecture-limited argument rejected")
    assert app.active_document() == definition
    assert app.choice(definition) == "x86"
    win32gui.PostMessage(hwnd, win32con.WM_CLOSE, 0, 0)
    n.wait(lambda: not n.is_window(hwnd), "failed binding cancelled")


def test_manual_winrt_binding_validates_arguments_and_reuses_instances(launch, settings):
    candidates = [settings[0].parent / "Windows.winmd",
                  Path(__file__).resolve().parents[2] / "inputs/Windows.winmd"]
    source = next((path for path in candidates if path.is_file()), None)
    if source is None:
        pytest.skip("Windows.winmd was not deployed")
    app = launch([source])
    app.query("Windows.Foundation.Collections.IVector`1")
    definition = app.open_row()
    app.choose(definition, "x64")
    dialog = bind_dialog(app, definition)
    hwnd = dialog.NativeWindowHandle
    field = n.get_item(hwnd, 1500)
    auto.ControlFromHandle(field).GetValuePattern().SetValue("int32, string")
    win32gui.PostMessage(hwnd, win32con.WM_COMMAND, 1231, n.get_item(hwnd, 1231))
    n.wait(lambda: bool(n.window_text(n.get_item(hwnd, 1232))), "argument validation error")
    assert app.active_document() == definition
    auto.ControlFromHandle(field).GetValuePattern().SetValue("int32")
    menu_key(field, win32con.VK_RETURN)
    integers = destination(app, definition, "IVector<int32>", "x64")
    n.wait(lambda: "b939af5b-b45d-5489-9149-61442c1905fe" in app.content(integers), "instance IID")
    assert "scalar ABI" in app.content(integers)
    assert "int32 GetAt(" in app.content(integers)
    pin_active_tab(app)
    dialog = bind_dialog(app, definition)
    hwnd = dialog.NativeWindowHandle
    auto.ControlFromHandle(n.get_item(hwnd, 1500)).GetValuePattern().SetValue("int32")
    win32gui.PostMessage(hwnd, win32con.WM_COMMAND, 1231, n.get_item(hwnd, 1231))
    assert destination(app, definition, "IVector<int32>", "x64") == integers
    dialog = bind_dialog(app, integers)
    hwnd = dialog.NativeWindowHandle
    auto.ControlFromHandle(n.get_item(hwnd, 1500)).GetValuePattern().SetValue("string")
    win32gui.PostMessage(hwnd, win32con.WM_COMMAND, 1231, n.get_item(hwnd, 1231))
    strings = destination(app, integers, "IVector<string>", "x64")
    assert strings != integers
    n.wait(lambda: "string GetAt(" in app.content(strings), "rebound instance signature")
    dialog = bind_dialog(app, strings)
    hwnd = dialog.NativeWindowHandle
    win32gui.PostMessage(hwnd, win32con.WM_CLOSE, 0, 0)
    n.wait(lambda: not n.is_window(hwnd), "binding cancelled")
    assert app.active_document() == strings


def test_separate_argument_fields_validate_and_prefill(launch, generic_metadata):
    app = launch([generic_metadata])
    app.query("Fixtures.Box`2")
    definition = app.open_row()
    app.choose(definition, "x64")
    dialog = bind_dialog(app, definition)
    hwnd = dialog.NativeWindowHandle
    first, second = (win32gui.GetDlgItem(hwnd, identifier) for identifier in (1500, 1501))
    assert n.window_text(first) == n.window_text(second) == ""
    auto.ControlFromHandle(first).GetValuePattern().SetValue("string")
    win32gui.PostMessage(hwnd, win32con.WM_COMMAND, 1231, n.get_item(hwnd, 1231))
    n.wait(lambda: bool(n.window_text(n.get_item(hwnd, 1232))), "empty second argument rejected")
    assert app.focused() == second
    assert n.window_text(first) == "string"
    auto.ControlFromHandle(second).GetValuePattern().SetValue("int32")
    menu_key(second, win32con.VK_RETURN)
    instance = destination(app, definition, "Box<string, int32>", "x64")
    n.wait(lambda: "string First;" in app.content(instance), "first argument substituted")
    assert "int32 Second;" in app.content(instance)
    dialog = bind_dialog(app, instance)
    hwnd = dialog.NativeWindowHandle
    assert n.window_text(n.get_item(hwnd, 1500)) == "string"
    assert n.window_text(n.get_item(hwnd, 1501)) == "int32"
    win32gui.PostMessage(hwnd, win32con.WM_CLOSE, 0, 0)
    n.wait(lambda: not n.is_window(hwnd), "prefilled form cancelled")
    app.query("Fixtures.BoxUses")
    nongeneric = app.open_row()
    assert not win32gui.IsWindowVisible(n.get_item(nongeneric, 1212))


@pytest.mark.parametrize("architecture", ["x86", "x64"])
def test_winrt_argument_errors_stay_in_form_and_focus_the_invalid_field(launch, settings, architecture):
    candidates = [settings[0].parent / "Windows.winmd", Path(__file__).resolve().parents[2] / "inputs/Windows.winmd"]
    source = next((path for path in candidates if path.is_file()), None)
    if source is None:
        pytest.skip("Windows.winmd was not deployed")
    app = launch([source])
    app.query("Windows.Foundation.Collections.IMap`2")
    definition = app.open_row()
    app.choose(definition, architecture)
    before = app.content(definition)
    dialog = bind_dialog(app, definition)
    hwnd = dialog.NativeWindowHandle
    first, second = (win32gui.GetDlgItem(hwnd, identifier) for identifier in (1500, 1501))
    auto.ControlFromHandle(first).GetValuePattern().SetValue("string")
    for expression in ("nuint", "nint", "IVector<nuint>", "IVector<IVector<nint>>", "int32[]"):
        auto.ControlFromHandle(second).GetValuePattern().SetValue(expression)
        n.send(hwnd, win32con.WM_COMMAND, 1231, win32gui.GetDlgItem(hwnd, 1231))
        assert n.is_window(hwnd)
        assert app.focused() == second
        assert n.window_text(first) == "string" and n.window_text(second) == expression
        assert expression in n.window_text(n.get_item(hwnd, 1232))
        assert app.active_document() == definition and app.choice(definition) == architecture
        assert app.content(definition) == before
    auto.ControlFromHandle(second).GetValuePattern().SetValue("uint64")
    menu_key(second, win32con.VK_RETURN)
    instance = destination(app, definition, "IMap<string, uint64>", architecture)
    n.wait(lambda: "uint64 Lookup(" in app.content(instance), "valid fixed-width WinRT argument accepted")


@pytest.mark.parametrize("architecture", ["x86", "x64"])
def test_dotnet_generics_keep_native_sized_arguments_without_conversion(launch, generic_metadata, architecture):
    app = launch([generic_metadata])
    app.query("Fixtures.Box`2")
    definition = app.open_row()
    app.choose(definition, architecture)
    dialog = bind_dialog(app, definition)
    hwnd = dialog.NativeWindowHandle
    for identifier, value in ((1500, "nint"), (1501, "nuint")):
        auto.ControlFromHandle(win32gui.GetDlgItem(hwnd, identifier)).GetValuePattern().SetValue(value)
    menu_key(win32gui.GetDlgItem(hwnd, 1501), win32con.VK_RETURN)
    instance = destination(app, definition, "Box<nint, nuint>", architecture)
    n.wait(lambda: "nint First;" in app.content(instance), "native signed argument preserved")
    assert "nuint Second;" in app.content(instance)
    dialog = bind_dialog(app, instance)
    hwnd = dialog.NativeWindowHandle
    assert n.window_text(n.get_item(hwnd, 1500)) == "nint"
    assert n.window_text(n.get_item(hwnd, 1501)) == "nuint"
    win32gui.PostMessage(hwnd, win32con.WM_CLOSE, 0, 0)
    n.wait(lambda: not n.is_window(hwnd), "native arguments prefilled")
    app.choose(instance, "x64" if architecture == "x86" else "x86")
    n.wait(lambda: "nint First;" in app.content(instance), "native argument survives architecture switch")
    assert "nuint Second;" in app.content(instance)


def test_many_arguments_scroll_keyboard_and_dpi_keep_values(launch, generic_metadata, settings):
    app = launch([generic_metadata])
    app.query("Fixtures.Many`8")
    definition = app.open_row()
    app.choose(definition, "x64")
    dialog = bind_dialog(app, definition)
    hwnd = dialog.NativeWindowHandle
    fields = [win32gui.GetDlgItem(hwnd, 1500 + index) for index in range(8)]
    apply = n.get_item(hwnd, 1231)
    for field in fields[:-1]:
        auto.ControlFromHandle(field).GetValuePattern().SetValue("int32")
    win32gui.PostMessage(hwnd, win32con.WM_COMMAND, 1231, apply)
    n.wait(lambda: app.focused() == fields[-1], "last invalid argument revealed and focused")
    assert n.is_visible(fields[-1])
    auto.ControlFromHandle(fields[-1]).GetValuePattern().SetValue("string")
    n.send(hwnd, win32con.WM_VSCROLL, win32con.SB_TOP, win32gui.GetDlgItem(hwnd, 1236))
    auto.ControlFromHandle(fields[0]).SetFocus()
    for current, following in zip(fields, fields[1:]):
        menu_key(current, win32con.VK_TAB)
        n.wait(lambda: app.focused() == following, "Tab reaches each argument")
        assert n.is_visible(following)
    observations = []
    for monitor in n.monitors():
        with n.window_dpi_context(hwnd):
            dialog.GetPattern(auto.PatternId.TransformPattern).Move(monitor[0] + 40, monitor[1] + 40)
            n.wait(lambda: n.window_text(fields[-1]) == "string", "input retained after monitor move")
            left, top = win32gui.ClientToScreen(hwnd, (0, 0))
            _, _, width, height = win32gui.GetClientRect(hwnd)
            for control in (fields[-1], apply, win32gui.GetDlgItem(hwnd, 2)):
                x1, y1, x2, y2 = n.rectangle(control)
                assert left <= x1 < x2 <= left + width
                assert top <= y1 < y2 <= top + height
            dpi = n.api(n.user, "GetDpiForWindow", n.w.UINT, n.w.HWND)(hwnd) if hasattr(n.user, "GetDpiForWindow") else None
            observations.append({"monitor": monitor, "effective_dpi": dpi, "bounds": n.rectangle(hwnd)})
    (settings[2] / "type-arguments-monitors.json").write_text(json.dumps(observations), encoding="utf-8")
    assert all(n.window_text(field) == "int32" for field in fields[:-1])
    win32gui.PostMessage(hwnd, win32con.WM_COMMAND, 1231, apply)
    instance = destination(app, definition, "Many<" + ", ".join(["int32"] * 7 + ["string"]) + ">", "x64")
    n.wait(lambda: "string Last;" in app.content(instance), "last argument substituted")
    assert "int32 First;" in app.content(instance)


def test_argument_form_simulated_pmv1_scaling_keeps_input_focus_and_scroll(launch, generic_metadata):
    if not hasattr(n.user, "GetDpiForWindow"):
        pytest.skip("per-monitor DPI is unavailable")
    app = launch([generic_metadata], dpi="pmv1")
    app.query("Fixtures.Many`8")
    definition = app.open_row()
    dialog = bind_dialog(app, definition)
    hwnd = dialog.NativeWindowHandle
    fields = [win32gui.GetDlgItem(hwnd, 1500 + index) for index in range(8)]
    apply = win32gui.GetDlgItem(hwnd, 1231)
    # An empty last field exposes the validation/reveal path before scaling.
    for field in fields[:-1]:
        auto.ControlFromHandle(field).GetValuePattern().SetValue("int32")
    n.send(hwnd, win32con.WM_COMMAND, 1231, apply)
    auto.ControlFromHandle(fields[-1]).GetValuePattern().SetValue("string")
    assert app.focused() == fields[-1]
    n.send(fields[-1], win32con.EM_SETSEL, 1, 4)
    with n.window_dpi_context(hwnd):
        original = n.rectangle(hwnd)
        actual = n.api(n.user, "GetDpiForWindow", n.w.UINT, n.w.HWND)(hwnd)
        for dpi in (96, 144, 192, actual):
            # Simulate only the PMv1 notification path, not an effective monitor change.
            rect = n.w.RECT(original[0], original[1],
                            original[0] + round((original[2] - original[0]) * dpi / actual),
                            original[1] + round((original[3] - original[1]) * dpi / actual))
            n.send(hwnd, 0x02E0, dpi | (dpi << 16), c.addressof(rect))
            assert app.focused() == fields[-1], (dpi, actual, win32gui.GetDlgCtrlID(app.focused()),
                [(win32gui.GetDlgCtrlID(field), bool(n.is_visible(field)), n.rectangle(field)) for field in fields],
                n.rectangle(win32gui.GetDlgItem(hwnd, 1234)), n.rectangle(win32gui.GetDlgItem(hwnd, 1235)))
            assert n.send(fields[-1], win32con.EM_GETSEL) == (4 << 16) | 1
            assert n.is_visible(fields[-1]) and not n.is_visible(fields[0])
            assert n.window_text(fields[-1]) == "string"
            assert all(n.window_text(field) == "int32" for field in fields[:-1])
            left, top = win32gui.ClientToScreen(hwnd, (0, 0))
            _, _, width, height = win32gui.GetClientRect(hwnd)
            for control in (fields[-1], apply, win32gui.GetDlgItem(hwnd, 2)):
                x1, y1, x2, y2 = n.rectangle(control)
                assert left <= x1 < x2 <= left + width
                assert top <= y1 < y2 <= top + height
    win32gui.PostMessage(hwnd, win32con.WM_CLOSE, 0, 0)
    n.wait(lambda: not n.is_window(hwnd), "scaled form cancelled")
