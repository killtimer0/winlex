"""Open application-owned dialogs through their user commands."""
from support import native as n


def open_about(app):
    app.command(1054)
    return n.wait(lambda: n.find_window(app.process.pid, '#32770', app.frame), 'About dialog', 15)


def open_help(app):
    app.command(1053)
    return n.wait(lambda: n.find_window(app.process.pid, '#32770', app.frame), 'search help dialog', 15)
