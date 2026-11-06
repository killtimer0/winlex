"""Control messages that pywin32's win32con does not define.

Values come from CommCtrl.h and WinUser.h.
"""
from win32con import WM_USER

TB_ISBUTTONCHECKED = WM_USER + 10
TB_GETHOTITEM = WM_USER + 71

LVM_FIRST = 0x1000
LVM_GETITEMSTATE = LVM_FIRST + 44
LVM_GETHOTITEM = LVM_FIRST + 61
LVM_GETITEMTEXTW = LVM_FIRST + 115
LVM_GETTOOLTIPS = LVM_FIRST + 78
MN_GETHMENU = 0x01E1  # WinUser.h: not derived from WM_USER
