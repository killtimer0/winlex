"""Apply/read user category choices through accessible settings controls."""
import uiautomation as auto
from support import deadline, native as n


@deadline.operation('set settings category mask via UIA')
def mask(app, sheet, value):
    page = app.settings_page(sheet, 1)
    for index in range(8):
        checkbox = auto.ControlFromHandle(n.get_item(page, 1310 + index))
        expected = (value >> index) & 1
        if checkbox.GetTogglePattern().ToggleState != expected:
            checkbox.GetTogglePattern().Toggle(waitTime=0)
            n.wait(lambda: checkbox.GetTogglePattern().ToggleState == expected,
                   "settings category toggle completed")
    return page


@deadline.operation('read settings category mask via UIA')
def checked(app, sheet):
    page = app.settings_page(sheet, 1)
    return sum(auto.ControlFromHandle(n.get_item(page, 1310 + index)).GetTogglePattern().ToggleState << index
               for index in range(8))
