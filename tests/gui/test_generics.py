"""Generic form behavior over the independent C# ReaderFixtures definitions."""
from pathlib import Path

from driver import until
import win32con
import win32gui


def open_definition(app):
    app.open_file(Path(__file__).with_name('fixtures') / 'ReaderFixtures.winmd')
    app.select_result('IBox`1')
    win32gui.SendMessage(app.frame, win32con.WM_COMMAND, 1050, 0)
    until('generic definition', lambda: 'IBox<T>' in app.editor().text_block()
          and 'Convert<U>' in app.editor().text_block())
    return app.editor()


def form(app):
    app.record('open type arguments')
    win32gui.PostMessage(app.document(), win32con.WM_COMMAND, 1212,
                         app.child(1212, app.document()))
    def opened():
        for dialog in app.windows('#32770'):
            if win32gui.IsWindowVisible(dialog):
                app.child(1500, dialog)
                return dialog
        return None
    return until('type argument dialog', opened)


def submit(app, dialog, argument):
    app.set_text(app.child(1500, dialog), argument)
    win32gui.PostMessage(dialog, win32con.WM_COMMAND, 1231, 0)


def cancel(dialog):
    win32gui.PostMessage(dialog, win32con.WM_COMMAND, win32con.IDCANCEL, 0)
    until('type argument dialog cancelled', lambda: not win32gui.IsWindow(dialog))


def state(document):
    return (document.text_block(), document.architecture(),
            document.selection_indices(), document.first_visible_line())


def test_generic_invalid_cancel_binding_and_history(app):
    original = open_definition(app)
    app.find('Value')
    original_state = state(original)
    dialog = form(app)
    submit(app, dialog, 'int32<')
    until('invalid argument diagnosed', lambda: app.text(app.child(1232, dialog)))
    assert app.text(app.child(1500, dialog)) == 'int32<'
    assert state(original) == original_state
    cancel(dialog)
    assert state(original) == original_state

    dialog = form(app)
    app.set_text(app.child(1500, dialog), 'string')
    cancel(dialog)
    assert state(original) == original_state
    dialog = form(app)
    submit(app, dialog, 'int32')
    until('argument accepted', lambda: not win32gui.IsWindow(dialog))
    until('substituted C# property', lambda: 'IBox<int32>' in app.editor().text_block()
          and 'int32 Value' in app.editor().text_block())
    bound = app.editor()
    assert 'Convert<U>' in bound.text_block() and 'int32 input' in bound.text_block()
    bound_state = state(bound)
    app.command(1055)
    until('unbound history restored', lambda: state(app.editor()) == original_state)
    app.command(1056)
    until('bound history restored', lambda: state(app.editor()) == bound_state)


def test_incompatible_generic_architecture_preserves_document_and_preference(app):
    original = open_definition(app)
    original.select_architecture('x64')
    until('x64 definition', lambda: original.architecture() == 'x64'
          and 'IBox<T>' in original.text_block())
    dialog = form(app)
    submit(app, dialog, 'Fixtures.X64Argument')
    until('architecture argument accepted', lambda: not win32gui.IsWindow(dialog))
    until('bound x64 argument', lambda: 'IBox<X64Argument>' in app.editor().text_block()
          and 'X64Argument Value' in app.editor().text_block())
    bound = app.editor()
    app.find('Value')
    before = state(bound)
    notice = app.text(app.child(1105))
    bound.select_architecture('x86')
    until('architecture rejection reported', lambda: app.text(app.child(1105))
          and app.text(app.child(1105)) != notice)
    assert state(bound) == before
    # Open-new uses the saved preference rather than inheriting the active view.
    fresh = app.open_symbol('Holder', new=True)
    assert fresh.architecture() == 'x64'
    app.command(1055)
    until('rejected switch adds no history visit', lambda: state(app.editor()) == before)
