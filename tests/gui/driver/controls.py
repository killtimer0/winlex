"""Native control observations shared by independent interaction scenarios."""
from support import native as n


class ComboInfo(n.c.Structure):
    _fields_ = [('size', n.w.DWORD), ('item', n.w.RECT), ('button', n.w.RECT),
                ('state', n.w.DWORD), ('combo', n.w.HWND), ('edit', n.w.HWND),
                ('list', n.w.HWND)]


def combo_info(hwnd):
    info = ComboInfo()
    info.size = n.c.sizeof(info)
    assert n.api(n.user, 'GetComboBoxInfo', n.w.BOOL, n.w.HWND,
                 n.c.POINTER(ComboInfo))(hwnd, n.c.byref(info))
    return info
