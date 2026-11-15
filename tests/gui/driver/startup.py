"""Explicit startup actions, separate from process/session custody."""
import win32con
from support import native as n
from support.session import invoke, message_button


def enter_workspace(session, profile, arguments):
    """Decline the empty-catalog guide when the scenario starts at the workspace.

    This is a user action, not session readiness. Guide scenarios instead request
    ``startup='metadata-guide'`` and own the prompt and its dismissal themselves.
    """
    if arguments or profile.get('Version') != 1 or profile.get('AutoLoad') not in (None, []):
        return
    prompt = n.wait(lambda: n.find_window(session.process.pid, '#32770', session.frame),
                    'empty-startup metadata guide prompt', 10)
    invoke(n.wait(lambda: message_button(prompt, win32con.IDNO), 'metadata prompt No button'))
    n.wait(lambda: not n.is_window(prompt), 'metadata guide prompt declined for workspace start')
