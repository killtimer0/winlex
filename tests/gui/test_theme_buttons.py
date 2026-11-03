"""Native button appearance and input contracts, independent of the paint path."""
import json

import pytest
import win32con
import win32gui

from support import native as n
from support.input_state import InputState
from test_about import open_about
from test_search_help import open_help
from test_theme_surfaces import background, center, colors, dark_app, foreground, move, screen_image

pytestmark = pytest.mark.interactive


def button_pixels(hwnd, checkbox=False, enabled=True):
    _, _, width, height = n.rectangle(hwnd, client=True)
    fill = background(hwnd, (4, 4, width - 4, height - 4))
    label = (height + 3 if checkbox else width // 5, height // 4,
             width - 4 if checkbox else width * 4 // 5, height * 3 // 4)
    palette = colors(hwnd, label)
    assert any(min(color) > (150 if enabled else 70) for color in palette), (
        'unreadable native button label', palette.most_common(5))
    bright_label = next(color for color, _ in palette.most_common()
                        if min(color) > (150 if enabled else 70))
    luminance = sum(channel * weight for channel, weight in
                    zip(bright_label, (0.0722, 0.7152, 0.2126)))
    return {'background': fill, 'label_colors': palette.most_common(5),
            'label_luminance': luminance}


@pytest.mark.parametrize('opener', [open_about, open_help], ids=['about', 'help'])
def test_dark_native_default_button_pixels_focus_and_enter(launch, profile, settings, opener):
    app = dark_app(launch, profile)
    with InputState() as inputs, foreground(app, inputs), n.window_dpi_context(app.frame):
        dialog = opener(app)
        button = n.get_item(dialog, 1)
        n.wait(lambda: win32gui.GetForegroundWindow() == dialog, 'default button dialog foreground')
        move(dialog, (8, 8))
        evidence = {'idle': button_pixels(button)}
        move(button, center(n.rectangle(button, client=True)))
        n.wait(lambda: n.send(button, win32con.BM_GETSTATE) & 0x200, 'native default button hot')
        evidence['hot'] = button_pixels(button)
        n.send(dialog, win32con.WM_NEXTDLGCTL, button, 1)
        n.wait(lambda: app.focused() == button, 'native default button focus')
        assert n.send(button, win32con.BM_GETSTATE) & win32con.BST_FOCUS
        screen_image(dialog, settings[2] / ('dark-native-default-%s.bmp' % opener.__name__))
        (settings[2] / ('dark-native-default-%s.json' % opener.__name__)).write_text(
            json.dumps(evidence, indent=2), encoding='utf-8')
        inputs.key(win32con.VK_RETURN)
        n.wait(lambda: not n.is_window(dialog), 'native default button Enter closes dialog')


def test_dark_native_find_buttons_and_auto_checkboxes(launch, profile, settings):
    app = dark_app(launch, profile, [settings[1]])
    app.query('CreateFileW')
    doc = app.open_row(app.named_rows('CreateFileW')[0])
    n.wait(lambda: 'CreateFileW(' in app.content(doc), 'find button document loaded', 45)
    evidence = {}
    with InputState() as inputs, foreground(app, inputs), n.window_dpi_context(app.frame):
        app.command(1060)
        query, count = n.get_item(doc, 1220), n.get_item(doc, 1223)
        n.wait(lambda: n.is_visible(query) and app.focused() == query, 'native find controls visible')
        n.set_text(query, 'HANDLE')
        n.wait(lambda: n.window_text(count).startswith('1/'), 'find initial match')
        for command, expected in [(1061, '2/'), (1062, '1/')]:
            button = n.get_item(doc, command)
            evidence[str(command)] = button_pixels(button)
            inputs.click(*win32gui.ClientToScreen(button, center(n.rectangle(button, client=True))))
            n.wait(lambda: n.window_text(count).startswith(expected), 'native find pushbutton navigates')
            assert app.focused() == button
        screen_image(app.frame, settings[2] / 'dark-native-find-buttons-enabled.bmp')
        for identifier in (1221, 1222):
            checkbox = n.get_item(doc, identifier)
            original = n.send(checkbox, win32con.BM_GETCHECK)
            evidence[str(identifier)] = {'unchecked': button_pixels(checkbox, checkbox=True)}
            _, _, width, height = n.rectangle(checkbox, client=True)
            glyph_area = (0, 0, height // 2 + 2, height)
            unchecked_glyph = colors(checkbox, glyph_area)
            point = win32gui.ClientToScreen(checkbox, center(n.rectangle(checkbox, client=True)))
            inputs.click(*point)
            n.wait(lambda: n.send(checkbox, win32con.BM_GETCHECK) != original, 'native checkbox mouse toggle')
            assert app.focused() == checkbox
            evidence[str(identifier)]['checked'] = button_pixels(checkbox, checkbox=True)
            def changed_glyph():
                palette = colors(checkbox, glyph_area)
                return palette if palette != unchecked_glyph else None

            checked_glyph = n.wait(changed_glyph, 'checkbox check state has a visible glyph')
            evidence[str(identifier)]['glyph_colors'] = {
                'unchecked': unchecked_glyph.most_common(5), 'checked': checked_glyph.most_common(5)}
            app.focus(query)
            label_area = (height + 3, 0, width, height)
            unfocused_label = colors(checkbox, label_area)
            for _ in range(12):
                previous = app.focused()
                inputs.key(win32con.VK_TAB)
                n.wait(lambda: app.focused() != previous, 'find keyboard Tab advances focus')
                if app.focused() == checkbox:
                    break
            assert app.focused() == checkbox
            assert n.send(checkbox, win32con.BM_GETSTATE) & win32con.BST_FOCUS
            def visible_focus_cue():
                palette = colors(checkbox, label_area)
                return palette if palette != unfocused_label else None

            focused_label = n.wait(visible_focus_cue, 'keyboard focus cue must be visible')
            evidence[str(identifier)]['focus_cue_colors'] = {
                'unfocused': unfocused_label.most_common(5), 'focused': focused_label.most_common(5)}
            screen_image(app.frame, settings[2] / ('dark-native-checkbox-%s-focused.bmp' % identifier))
            assert focused_label != unfocused_label, 'keyboard focus cue must be visible'
            inputs.key(win32con.VK_SPACE)
            n.wait(lambda: n.send(checkbox, win32con.BM_GETCHECK) == original, 'native checkbox Space toggle')
        checkbox = n.get_item(doc, 1221)
        enable = n.api(n.user, 'EnableWindow', n.w.BOOL, n.w.HWND, n.w.BOOL)
        original = n.send(checkbox, win32con.BM_GETCHECK)
        try:
            app.focus(query)
            move(query, center(n.rectangle(query, client=True)))
            evidence['enabled_before_disable'] = button_pixels(checkbox, checkbox=True)
            enable(checkbox, False)
            assert not win32gui.IsWindowEnabled(checkbox)
            def dimmed_label():
                sample = button_pixels(checkbox, checkbox=True, enabled=False)
                evidence['disabled'] = sample
                return sample if sample['label_luminance'] < (
                    evidence['enabled_before_disable']['label_luminance'] * 0.9) else None

            try:
                n.wait(dimmed_label, 'disabled checkbox label visibly dimmer than enabled label')
            finally:
                screen_image(app.frame, settings[2] / 'dark-native-checkbox-disabled.bmp')
                (settings[2] / 'dark-native-find-buttons.json').write_text(
                    json.dumps(evidence, indent=2), encoding='utf-8')
            inputs.click(*win32gui.ClientToScreen(checkbox, center(n.rectangle(checkbox, client=True))))
            assert n.send(checkbox, win32con.BM_GETCHECK) == original
            assert app.focused() == query
            before_space = n.window_text(query)
            inputs.key(win32con.VK_SPACE)
            n.wait(lambda: n.window_text(query) != before_space, 'Space reaches enabled focus, not disabled checkbox')
            assert n.send(checkbox, win32con.BM_GETCHECK) == original
            screen_image(app.frame, settings[2] / 'dark-native-find-buttons.bmp')
        finally:
            enable(checkbox, True)
        (settings[2] / 'dark-native-find-buttons.json').write_text(
            json.dumps(evidence, indent=2), encoding='utf-8')
        close = n.get_item(doc, 1063)
        button_pixels(close)
        inputs.click(*win32gui.ClientToScreen(close, center(n.rectangle(close, client=True))))
        n.wait(lambda: not n.is_visible(query), 'native find Close button')
