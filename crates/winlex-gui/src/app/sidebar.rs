//! Independent virtual lists retain selection and scroll while panels are hidden.
use super::{
    bind,
    catalog::{self, Panel},
    dpi, font_for, read, sources, write,
};
use crate::{
    layout,
    native::*,
    resources::{self, *},
};
use std::ptr;
pub(super) const TOOLBAR_WIDTH: i32 = 72;
pub(super) fn focus_target() -> HWND {
    read(|a| {
        if a.config.collapsed {
            a.toolbar[usize::from(a.session.panel == Panel::Sources)]
        } else if a.session.panel == Panel::Sources {
            a.source_list
        } else {
            a.search
        }
    })
}
pub(super) fn create(frame: HWND) -> bool {
    let search = child(
        frame,
        "EDIT",
        "",
        WS_TABSTOP | ES_AUTOHSCROLL as u32,
        SEARCH,
    );
    let lists = [RESULTS, SOURCE_LIST].map(|id| {
        let h = child(
            frame,
            "SysListView32",
            "",
            WS_TABSTOP | LVS_REPORT | LVS_OWNERDATA | LVS_SINGLESEL | LVS_NOCOLUMNHEADER,
            id,
        );
        list_column(h, 0, "", 100);
        send(
            h,
            LVM_SETEXTENDEDLISTVIEWSTYLE,
            0,
            (LVS_EX_FULLROWSELECT | LVS_EX_INFOTIP | LVS_EX_DOUBLEBUFFER) as isize,
        );
        h
    });
    let toolbar = [
        (PANEL_SEARCH, IDS_PANEL_SEARCH),
        (MANAGE_SOURCES, IDS_PANEL_SOURCES),
    ]
    .map(|(id, label)| child(frame, "BUTTON", &resources::text(label), WS_TABSTOP, id));
    let tools = [
        (LOAD_METADATA, IDS_ADD),
        (REFRESH_SOURCES, IDS_REFRESH),
        (ADD_DIRECTORY, IDS_DIRECTORY),
        (FORGET_LOCATION, IDS_FORGET_LOCATION),
    ]
    .map(|(id, label)| child(frame, "BUTTON", &resources::text(label), WS_TABSTOP, id));
    let splitter = child(frame, "Winlex.Splitter", "", 0, 0);
    write(|a| {
        a.search = search;
        a.results = lists[0];
        a.source_list = lists[1];
        a.toolbar = toolbar;
        a.source_tools = tools;
        a.splitter = splitter;
    });
    unsafe {
        set_subclass(search, Some(edit_proc), 1, 0) != 0
            && lists
                .iter()
                .all(|&h| set_subclass(h, Some(list_proc), 1, 0) != 0)
            && [search, splitter]
                .into_iter()
                .chain(lists)
                .chain(toolbar)
                .chain(tools)
                .all(|h| !h.is_null())
    }
}
pub(super) fn show(panel: Panel, toggle: bool) {
    write(|a| {
        a.config.collapsed = toggle && a.session.panel == panel && !a.config.collapsed;
        a.session.panel = panel;
    });
    super::frame::layout_frame();
    unsafe {
        let focused = GetFocus();
        if !focused.is_null() && IsWindowVisible(focused) == 0 {
            focus(focus_target());
        }
    }
}
pub(super) fn prepare() {
    let (frame, search, lists, toolbar, tools) = read(|a| {
        (
            a.frame,
            a.search,
            [a.results, a.source_list],
            a.toolbar,
            a.source_tools,
        )
    });
    let value = dpi().window(frame);
    let (font, height) = font_for(value);
    for h in [search]
        .into_iter()
        .chain(lists)
        .chain(toolbar)
        .chain(tools)
    {
        bind(h, font);
    }
    for h in lists {
        // A transparent small image defines a fixed two-line native row height.
        unsafe {
            let image =
                ImageList_Create(1, height * 2 + layout::scale(8, value), ILC_COLOR32, 1, 1);
            let old = send(h, LVM_SETIMAGELIST, LVSIL_SMALL as usize, image as isize);
            if old != 0 {
                ImageList_Destroy(old as _);
            }
        }
    }
}
pub(super) fn fit(h: HWND) {
    let width = client(h).right.max(1);
    send(h, LVM_SETCOLUMNWIDTH, 0, width as isize);
    unsafe {
        ShowScrollBar(h, SB_HORZ, 0);
    }
}
pub(super) fn layout(bottom: i32) -> i32 {
    let (frame, search, results, sources, toolbar, tools, splitter, width, panel, collapsed) =
        read(|a| {
            (
                a.frame,
                a.search,
                a.results,
                a.source_list,
                a.toolbar,
                a.source_tools,
                a.splitter,
                a.sidebar,
                a.session.panel,
                a.config.collapsed,
            )
        });
    let value = dpi().window(frame);
    let s = |v| layout::scale(v, value);
    let (_, height) = font_for(value);
    let row = height + s(12);
    let x = s(TOOLBAR_WIDTH);
    for (i, h) in toolbar.into_iter().enumerate() {
        place(
            h,
            layout::rect(s(3), s(4) + (row + s(4)) * i as i32, x - s(6), row),
        );
    }
    let width = s(width).min((client(frame).right - x - s(160)).max(0));
    let visible = !collapsed;
    unsafe {
        ShowWindow(
            search,
            if visible && panel == Panel::Results {
                SW_SHOW
            } else {
                SW_HIDE
            },
        );
        ShowWindow(
            results,
            if visible && panel == Panel::Results {
                SW_SHOW
            } else {
                SW_HIDE
            },
        );
        ShowWindow(
            sources,
            if visible && panel == Panel::Sources {
                SW_SHOW
            } else {
                SW_HIDE
            },
        );
        ShowWindow(splitter, if visible { SW_SHOW } else { SW_HIDE });
        for h in tools {
            ShowWindow(
                h,
                if visible && panel == Panel::Sources {
                    SW_SHOW
                } else {
                    SW_HIDE
                },
            );
        }
    }
    if collapsed {
        return x;
    }
    place(search, layout::rect(x + s(4), s(4), width - s(8), row));
    for (i, h) in tools.into_iter().enumerate() {
        place(
            h,
            layout::rect(
                x + width * (i as i32 % 2) / 2,
                s(4) + row * (i as i32 / 2),
                width / 2,
                row,
            ),
        );
    }
    for (h, rows) in [(results, 1), (sources, 2)] {
        let top = rows * row + s(8);
        place(h, layout::rect(x, top, width, bottom - top));
        fit(h);
    }
    place(splitter, layout::rect(x + width, 0, s(5), bottom));
    x + width + s(5)
}
fn cell(source: bool, row: usize, detail: bool) -> String {
    if source {
        sources::cell(row, detail)
    } else {
        catalog::cell(row, detail)
    }
}
unsafe fn copy_text(value: &str, buffer: *mut u16, capacity: i32) {
    if !buffer.is_null() && capacity > 0 {
        let s = wide(value);
        let n = (capacity as usize - 1).min(s.len() - 1);
        unsafe {
            ptr::copy_nonoverlapping(s.as_ptr(), buffer, n);
            *buffer.add(n) = 0;
        }
    }
}
pub(super) unsafe fn notify(l: isize) -> Option<isize> {
    unsafe {
        let hdr = &*(l as *const NMHDR);
        if hdr.idFrom != RESULTS as usize && hdr.idFrom != SOURCE_LIST as usize {
            return None;
        }
        let source = hdr.idFrom == SOURCE_LIST as usize;
        match hdr.code {
            LVN_GETDISPINFOW => {
                let i = &mut *(l as *mut NMLVDISPINFOW);
                if i.item.mask & LVIF_TEXT != 0 {
                    copy_text(
                        &cell(source, i.item.iItem as usize, true),
                        i.item.pszText,
                        i.item.cchTextMax,
                    );
                }
                Some(0)
            }
            LVN_GETINFOTIPW => {
                let i = &mut *(l as *mut NMLVGETINFOTIPW);
                copy_text(
                    &cell(source, i.iItem as usize, true),
                    i.pszText,
                    i.cchTextMax,
                );
                Some(0)
            }
            NM_CLICK | NM_DBLCLK if source || hdr.code == NM_DBLCLK => {
                let i = &*(l as *const NMITEMACTIVATE);
                if i.iItem >= 0 {
                    if source {
                        sources::open_row(i.iItem as isize);
                    } else {
                        catalog::open_row(i.iItem as isize);
                    }
                }
                Some(0)
            }
            NM_CUSTOMDRAW => {
                let d = &mut *(l as *mut NMLVCUSTOMDRAW);
                match d.nmcd.dwDrawStage {
                    CDDS_PREPAINT => Some(CDRF_NOTIFYITEMDRAW as isize),
                    CDDS_ITEMPREPAINT => {
                        let selected = send(
                            hdr.hwndFrom,
                            LVM_GETITEMSTATE,
                            d.nmcd.dwItemSpec,
                            LVIS_SELECTED as isize,
                        ) != 0;
                        let bg = if selected {
                            COLOR_HIGHLIGHT
                        } else {
                            COLOR_WINDOW
                        };
                        let fg = if selected {
                            COLOR_HIGHLIGHTTEXT
                        } else {
                            COLOR_WINDOWTEXT
                        };
                        FillRect(d.nmcd.hdc, &d.nmcd.rc, GetSysColorBrush(bg));
                        SetTextColor(d.nmcd.hdc, GetSysColor(fg));
                        SetBkMode(d.nmcd.hdc, TRANSPARENT as i32);
                        let content = cell(source, d.nmcd.dwItemSpec, false);
                        let mut lines = content.splitn(2, '\n');
                        let mut r = d.nmcd.rc;
                        r.left += layout::scale(4, dpi().window(hdr.hwndFrom));
                        let mid = (r.top + r.bottom) / 2;
                        r.bottom = mid;
                        let mut first = wide(lines.next().unwrap_or(""));
                        DrawTextW(
                            d.nmcd.hdc,
                            first.as_mut_ptr(),
                            -1,
                            &mut r,
                            DT_SINGLELINE | DT_END_ELLIPSIS | DT_NOPREFIX | DT_VCENTER,
                        );
                        r.top = mid;
                        r.bottom = d.nmcd.rc.bottom;
                        let mut second = wide(lines.next().unwrap_or(""));
                        DrawTextW(
                            d.nmcd.hdc,
                            second.as_mut_ptr(),
                            -1,
                            &mut r,
                            DT_SINGLELINE | DT_END_ELLIPSIS | DT_NOPREFIX | DT_VCENTER,
                        );
                        if d.nmcd.uItemState & CDIS_FOCUS != 0 {
                            DrawFocusRect(d.nmcd.hdc, &d.nmcd.rc);
                        }
                        Some(CDRF_SKIPDEFAULT as isize)
                    }
                    _ => Some(CDRF_DODEFAULT as isize),
                }
            }
            _ => None,
        }
    }
}
unsafe extern "system" fn edit_proc(
    h: HWND,
    msg: u32,
    w: usize,
    l: isize,
    _id: usize,
    _data: usize,
) -> isize {
    unsafe {
        if msg == WM_IME_STARTCOMPOSITION {
            write(|a| a.composing = true);
            catalog::input_changed();
        }
        let result = default_subclass(h, msg, w, l);
        if msg == WM_IME_ENDCOMPOSITION {
            write(|a| a.composing = false);
            catalog::input_changed();
        }
        if msg == WM_NCDESTROY {
            remove_subclass(h, Some(edit_proc), 1);
        }
        result
    }
}
unsafe extern "system" fn list_proc(
    h: HWND,
    msg: u32,
    w: usize,
    l: isize,
    _id: usize,
    _data: usize,
) -> isize {
    unsafe {
        if msg == WM_KEYDOWN && w == VK_F1 as usize {
            let row = send(h, LVM_GETNEXTITEM, usize::MAX, LVNI_SELECTED as isize);
            if row >= 0 {
                let content = cell(h == read(|a| a.source_list), row as usize, true);
                MessageBoxW(
                    h,
                    wide(&content).as_ptr(),
                    wide(&resources::text(IDS_SOURCE)).as_ptr(),
                    MB_OK | MB_ICONINFORMATION,
                );
            }
            return 0;
        }
        if msg == WM_NCDESTROY {
            let image = send(h, LVM_SETIMAGELIST, LVSIL_SMALL as usize, 0);
            if image != 0 {
                ImageList_Destroy(image as _);
            }
            remove_subclass(h, Some(list_proc), 1);
        }
        default_subclass(h, msg, w, l)
    }
}
