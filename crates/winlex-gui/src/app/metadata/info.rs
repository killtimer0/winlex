use super::Binding;
use crate::{
    dpi::Font,
    layout,
    native::*,
    resources::{self, *},
};
use std::{cell::RefCell, ffi::OsStr, os::windows::ffi::OsStrExt, path::PathBuf, ptr};
use windows::{
    Win32::{
        Foundation::HWND as ComHwnd,
        System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance},
        UI::Accessibility::{CLSID_AccPropServices, IAccPropServices, PROPID_ACC_NAME},
    },
    core::PCWSTR,
};
use windows_sys::Win32::System::SystemServices::SS_ETCHEDHORZ;

const KINDS: &[(&str, u16)] = &[
    ("class", IDS_KIND_CLASS),
    ("interface", IDS_KIND_INTERFACE),
    ("struct", IDS_KIND_STRUCT),
    ("enum", IDS_KIND_ENUM),
    ("delegate", IDS_KIND_DELEGATE),
    ("attribute", IDS_KIND_ATTRIBUTE),
    ("module", IDS_KIND_MODULE),
    ("method", IDS_KIND_METHOD),
    ("constant", IDS_KIND_CONSTANT),
    ("field", IDS_KIND_FIELD),
    ("property", IDS_KIND_PROPERTY),
    ("event", IDS_KIND_EVENT),
    ("type_reference", IDS_KIND_REFERENCE),
];

#[derive(Clone)]
struct Snapshot {
    name: String,
    full_name: String,
    path: PathBuf,
    id: winlex_core::EntityId,
    online_target: String,
}
impl Snapshot {
    fn copies(&self) -> Vec<(u16, Vec<u16>)> {
        let id = self.id.to_string();
        [
            (IDS_COPY_SOURCE_PATH, self.path.as_os_str()),
            (IDS_COPY_ENTITY_ID, OsStr::new(&id)),
            (IDS_COPY_SYMBOL_NAME, OsStr::new(&self.name)),
            (IDS_COPY_QUALIFIED_NAME, OsStr::new(&self.full_name)),
            (
                IDS_COPY_SOURCE_NAME,
                self.path.file_name().unwrap_or_default(),
            ),
        ]
        .into_iter()
        .map(|(label, value)| (label, value.encode_wide().collect()))
        .collect()
    }
}

#[derive(Default)]
pub(super) struct Info {
    editor: HWND,
    name: HWND,
    kind: HWND,
    scope: HWND,
    source_label: HWND,
    source: HWND,
    architecture_label: HWND,
    pub architecture: HWND,
    more: HWND,
    online: HWND,
    line: HWND,
    heading: RefCell<Option<Font>>,
    snapshot: RefCell<Option<Snapshot>>,
    accessibility: RefCell<Option<IAccPropServices>>,
}
impl Info {
    pub fn create(&mut self, owner: HWND, editor: HWND) -> anyhow::Result<()> {
        self.editor = editor;
        let edit = |id| unsafe {
            child(
                owner,
                w!("EDIT"),
                &[0],
                ES_READONLY as u32 | ES_AUTOHSCROLL as u32,
                id,
            )
        };
        self.name = edit(DOCUMENT_NAME)?;
        self.kind = unsafe { child(owner, w!("STATIC"), &[0], 0, DOCUMENT_KIND) }?;
        self.scope = edit(DOCUMENT_SCOPE)?;
        self.source = edit(DOCUMENT_SOURCE)?;
        for (control, label) in [
            (&mut self.source_label, IDS_DOCUMENT_SOURCE),
            (&mut self.architecture_label, IDS_ARCHITECTURE),
        ] {
            *control = unsafe { child(owner, w!("STATIC"), resources::wide_text(label), 0, 0) }?;
        }
        self.architecture = unsafe {
            child(
                owner,
                w!("COMBOBOX"),
                &[0],
                WS_TABSTOP | WS_VSCROLL | CBS_DROPDOWNLIST as u32,
                SELECT_ARCH,
            )
        }?;
        for (control, label, id) in [
            (&mut self.more, IDS_DOCUMENT_MORE, DOCUMENT_MORE),
            (&mut self.online, IDS_ONLINE_SEARCH, DOCUMENT_ONLINE),
        ] {
            *control = unsafe {
                child(
                    owner,
                    w!("BUTTON"),
                    resources::wide_text(label),
                    WS_TABSTOP,
                    id,
                )
            }?;
        }
        self.line = unsafe { child(owner, w!("STATIC"), &[0], SS_ETCHEDHORZ, 0) }?;
        let services: IAccPropServices =
            unsafe { CoCreateInstance(&CLSID_AccPropServices, None, CLSCTX_INPROC_SERVER)? };
        self.accessibility.replace(Some(services.clone()));
        for (control, label) in self.names(editor) {
            if let Err(error) = unsafe {
                services.SetHwndPropStr(
                    ComHwnd(control),
                    OBJID_CLIENT as u32,
                    CHILDID_SELF,
                    PROPID_ACC_NAME,
                    PCWSTR(resources::wide_text(label).as_ptr()),
                )
            } {
                self.clear_accessibility();
                return Err(error.into());
            }
        }
        Ok(())
    }
    fn names(&self, editor: HWND) -> [(HWND, u16); 5] {
        [
            (self.name, IDS_DOCUMENT_NAME),
            (self.scope, IDS_DOCUMENT_SCOPE),
            (self.source, IDS_DOCUMENT_SOURCE),
            (self.architecture, IDS_ARCHITECTURE),
            (editor, IDS_DOCUMENT_CODE),
        ]
    }
    pub fn clear_accessibility(&self) {
        let services = self.accessibility.borrow_mut().take();
        if let Some(services) = services {
            for (control, _) in self.names(self.editor) {
                unsafe {
                    let _ = services.ClearHwndProps(
                        ComHwnd(control),
                        OBJID_CLIENT as u32,
                        CHILDID_SELF,
                        &[PROPID_ACC_NAME],
                    );
                }
            }
        }
    }
    pub fn fill(&self, binding: &Binding) -> anyhow::Result<()> {
        let entity = binding.catalog.entity(binding.id)?;
        let context = binding.catalog.entity_context(binding.id)?;
        let scope = match context.owner {
            Some(owner) if !context.namespace.is_empty() => {
                format!("{} · {owner}", context.namespace)
            }
            Some(owner) => owner,
            None => context.namespace,
        };
        set_text(self.name, &entity.name);
        set_text(self.scope, &scope);
        set_text(self.kind, &resources::text(kind_label(&entity.kind)));
        let documentation = binding
            .catalog
            .declaration(binding.id, Some(binding.architecture))?
            .documentation_url();
        let (label, target) = online_target(documentation.as_deref(), &entity.full_name);
        set_text(self.online, &resources::text(label));
        let path = binding.catalog.sources()[binding.id.source_id].path.clone();
        let filename: Vec<u16> = path
            .file_name()
            .unwrap_or_default()
            .encode_wide()
            .chain(Some(0))
            .collect();
        send(self.source, WM_SETTEXT, 0, filename.as_ptr() as isize);
        self.snapshot.replace(Some(Snapshot {
            name: entity.name,
            full_name: entity.full_name,
            path,
            id: binding.id,
            online_target: target,
        }));
        Ok(())
    }
    pub fn prepare(&self, m: &layout::Metrics) -> anyhow::Result<()> {
        for control in [
            self.kind,
            self.scope,
            self.source_label,
            self.source,
            self.architecture_label,
            self.architecture,
            self.more,
            self.online,
        ] {
            super::super::bind(control, m.font);
        }
        let heading = Font::heading(m.dpi, m.font)?;
        super::super::bind(self.name, heading.handle.0);
        self.heading.replace(Some(heading));
        Ok(())
    }
    pub fn color(&self, control: HWND, dc: HDC) -> Option<isize> {
        if [
            self.name,
            self.kind,
            self.scope,
            self.source_label,
            self.source,
            self.architecture_label,
        ]
        .contains(&control)
        {
            unsafe {
                SetTextColor(dc, GetSysColor(COLOR_BTNTEXT));
                SetBkColor(dc, GetSysColor(COLOR_BTNFACE));
                Some(GetSysColorBrush(COLOR_BTNFACE) as isize)
            }
        } else {
            None
        }
    }
    pub fn layout(&self, area: &mut RECT, m: &layout::Metrics, positions: &mut Vec<(HWND, RECT)>) {
        let button = m.button_size(self.more);
        let online = m.button_size(self.online);
        let combo = bounds(self.architecture, ptr::null_mut());
        let combo_height = combo.bottom - combo.top;
        let label_width =
            m.label_width([resources::wide_text(IDS_ARCHITECTURE)].into_iter(), 0) - m.px(16);
        let actions_width = label_width + m.px(70 + 8) + button.cx + m.px(8) + online.cx;
        let wrap_button = actions_width > area.right - area.left;
        let row_height = m.height.max(combo_height).max(if wrap_button {
            0
        } else {
            button.cy.max(online.cy)
        });
        let heading_height = self
            .heading
            .borrow()
            .as_ref()
            .map_or(m.height, |font| font.height);
        let kind_width = m.label_width([text_wide(self.kind)].into_iter(), 0);
        let wide = area.right - area.left >= m.px(520).max(actions_width + m.px(180));
        let mut title = layout::take_top(area, heading_height + m.px(4));
        if wide {
            let left = (title.right - kind_width).max(title.left);
            positions.push((
                self.kind,
                layout::rect(
                    left,
                    title.top,
                    title.right - left,
                    (title.bottom - title.top).min(m.height),
                ),
            ));
            title.right = (left - m.px(8)).max(title.left);
        } else {
            positions.push((self.kind, layout::take_top(area, m.height + m.px(4))));
        }
        positions.push((self.name, title));
        positions.push((self.scope, layout::take_top(area, m.height + m.px(8))));
        let mut row = layout::take_top(area, if wide { row_height } else { m.height });
        layout::take_top(area, m.px(4));
        let source_width =
            m.label_width([resources::wide_text(IDS_DOCUMENT_SOURCE)].into_iter(), 0) - m.px(16);
        let mut source_row = if wide {
            let width = (row.right - row.left - actions_width - m.px(16)).max(0);
            let source = layout::take_left(&mut row, width);
            layout::take_left(&mut row, m.px(16));
            source
        } else {
            row
        };
        positions.push((
            self.source_label,
            centered(layout::take_left(&mut source_row, source_width), m.height),
        ));
        positions.push((self.source, centered(source_row, m.height)));
        if !wide {
            row = layout::take_top(area, row_height);
            layout::take_top(area, m.px(4));
        }
        positions.push((
            self.architecture_label,
            centered(layout::take_left(&mut row, label_width), m.height),
        ));
        let selector = centered(layout::take_left(&mut row, m.px(70)), combo_height);
        show(
            self.architecture,
            selector.bottom - selector.top >= combo_height,
        );
        positions.push((
            self.architecture,
            layout::rect(
                selector.left,
                selector.top,
                selector.right - selector.left,
                m.row() * 6,
            ),
        ));
        if wrap_button {
            row = layout::take_top(area, button.cy.max(online.cy));
            layout::take_top(area, m.px(4));
        } else {
            layout::take_left(&mut row, m.px(8));
        }
        place_button(self.more, &mut row, button, positions);
        if row.right - row.left < m.px(8) + online.cx {
            row = layout::take_top(area, online.cy);
            layout::take_top(area, m.px(4));
        } else {
            layout::take_left(&mut row, m.px(8));
        }
        place_button(self.online, &mut row, online, positions);
        positions.push((self.line, layout::take_top(area, m.px(2))));
        layout::take_top(area, m.px(8));
    }
    pub fn open_online(&self, owner: HWND) -> anyhow::Result<()> {
        let Some(target) = self
            .snapshot
            .borrow()
            .as_ref()
            .map(|s| s.online_target.clone())
        else {
            return Ok(());
        };
        let result = unsafe {
            windows_sys::Win32::UI::Shell::ShellExecuteW(
                owner,
                w!("open"),
                wide(&target).as_ptr(),
                ptr::null(),
                ptr::null(),
                SW_SHOWNORMAL,
            )
        } as isize;
        anyhow::ensure!(result > 32, "Open online reference failed ({result})");
        Ok(())
    }
    pub fn menu(&self, owner: HWND) -> anyhow::Result<()> {
        let Some(snapshot) = self.snapshot.borrow().clone() else {
            return Ok(());
        };
        let copies = snapshot.copies();
        unsafe {
            let menu = Popup::new()?;
            let m = super::super::metrics(owner);
            with_window_font(owner, m.font, |dc| {
                if dc.is_null() {
                    return Err(error("Measure document information menu"));
                }
                let measure = |value: &[u16]| {
                    let mut size = SIZE { cx: 0, cy: 0 };
                    GetTextExtentPoint32W(dc, value.as_ptr(), value.len() as i32, &mut size);
                    size.cx
                };
                let mut monitor: MONITORINFO = std::mem::zeroed();
                monitor.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
                if GetMonitorInfoW(
                    MonitorFromWindow(owner, MONITOR_DEFAULTTONEAREST),
                    &mut monitor,
                ) == 0
                {
                    return Err(error("Measure information menu work area"));
                }
                let width = m
                    .px(420)
                    .min((monitor.rcWork.right - monitor.rcWork.left) / 2);
                for (id, value) in &copies {
                    if *id == IDS_COPY_SYMBOL_NAME {
                        menu.separator()?;
                    }
                    let kind = if *id == IDS_COPY_SOURCE_PATH {
                        crate::list::FoldKind::Path
                    } else {
                        crate::list::FoldKind::Text
                    };
                    let segment = crate::list::TextSegment::new(value.iter().copied(), kind);
                    let shown = segment.fold(width, &measure).map(|(shown, _)| shown);
                    let label =
                        menu_label(resources::wide_text(*id), shown.as_deref().unwrap_or(value));
                    menu.item(*id, &label, true)?;
                }
                Ok(())
            })?;
            let button = bounds(self.more, ptr::null_mut());
            let selected = menu.track(
                owner,
                POINT {
                    x: button.left,
                    y: button.bottom,
                },
            );
            if IsWindow(owner) != 0
                && let Some((_, value)) = copies.iter().find(|(id, _)| *id == selected)
            {
                copy_unicode(owner, value)?;
            }
        }
        Ok(())
    }
}
impl Drop for Info {
    fn drop(&mut self) {
        self.clear_accessibility();
    }
}
fn online_target(documentation: Option<&str>, name: &str) -> (u16, String) {
    if let Some(url) = documentation.filter(|url| {
        let Some(rest) = url
            .get(..8)
            .filter(|scheme| scheme.eq_ignore_ascii_case("https://"))
            .and_then(|_| url.get(8..))
        else {
            return false;
        };
        let host = rest.split(['/', '?', '#']).next().unwrap_or_default();
        !host.is_empty()
            && host
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b':'))
            && host.bytes().any(|b| b.is_ascii_alphanumeric())
            && !url
                .chars()
                .any(|ch| ch.is_control() || ch.is_whitespace() || matches!(ch, '\\' | '"' | '\''))
    }) {
        return (IDS_OFFICIAL_DOCUMENTATION, url.to_owned());
    }
    let mut url = String::from("https://www.google.com/search?q=");
    for byte in name.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            url.push(byte as char);
        } else {
            use std::fmt::Write;
            write!(url, "%{byte:02X}").unwrap();
        }
    }
    (IDS_ONLINE_SEARCH, url)
}
fn kind_label(kind: &str) -> u16 {
    KINDS
        .iter()
        .find(|(value, _)| *value == kind)
        .map_or(IDS_KIND_UNKNOWN, |(_, label)| *label)
}
fn centered(mut row: RECT, height: i32) -> RECT {
    row.top += ((row.bottom - row.top - height) / 2).max(0);
    row.bottom = (row.top + height).min(row.bottom);
    row
}
fn place_button(control: HWND, row: &mut RECT, size: SIZE, positions: &mut Vec<(HWND, RECT)>) {
    let rect = centered(layout::take_left(row, size.cx), size.cy);
    show(
        control,
        rect.right - rect.left >= size.cx && rect.bottom - rect.top >= size.cy,
    );
    positions.push((control, rect));
}
fn menu_label(label: &[u16], value: &[u16]) -> Vec<u16> {
    let mut result = Vec::new();
    let label = label.iter().copied().take_while(|ch| *ch != 0);
    let value = value
        .iter()
        .copied()
        .map(|ch| if matches!(ch, 9 | 10 | 13) { 32 } else { ch });
    for ch in label.chain(Some(9)).chain(value) {
        result.push(ch);
        if ch == 38 {
            result.push(ch);
        }
    }
    result.push(0);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn menu_labels_escape_mnemonics_and_keep_values_on_one_line() {
        let value: Vec<_> = "A&B\tC\r\nD".encode_utf16().collect();
        assert_eq!(
            String::from_utf16(&menu_label(&wide("Copy & path"), &value)).unwrap(),
            "Copy && path\tA&&B C  D\0"
        );
    }
    #[test]
    fn online_targets_use_https_documentation_or_encoded_search() {
        let url = "https://learn.microsoft.com/windows/win32/api/test?a=1&b=2#member";
        assert_eq!(
            online_target(Some(url), "ignored"),
            (IDS_OFFICIAL_DOCUMENTATION, url.into())
        );
        for invalid in [
            None,
            Some(""),
            Some("http://example.com"),
            Some("file:///test"),
            Some("javascript:alert(1)"),
            Some("https:///test"),
            Some("https://example.com\0.exe"),
            Some("https://example.com/a b"),
            Some("https://example.com\\test"),
            Some("https://user@example.com"),
        ] {
            assert_eq!(
                online_target(invalid, "N.A&B 中"),
                (
                    IDS_ONLINE_SEARCH,
                    "https://www.google.com/search?q=N.A%26B%20%E4%B8%AD".into()
                )
            );
        }
    }
    #[test]
    fn information_copies_keep_native_paths_and_ids() {
        use std::ffi::OsString;
        use std::os::windows::ffi::OsStringExt;
        for prefix in [
            r"C:\metadata\",
            r"\\server\share\",
            r"\\?\C:\metadata\",
            r"\\?\UNC\server\share\",
        ] {
            let mut path: Vec<_> = format!("{prefix}{}\\source&", "directory\\".repeat(40))
                .encode_utf16()
                .collect();
            path.extend([0xD800]);
            path.extend(".winmd".encode_utf16());
            let snapshot = Snapshot {
                name: "A&B".into(),
                full_name: "N.A&B".into(),
                online_target: online_target(None, "N.A&B").1,
                path: PathBuf::from(OsString::from_wide(&path)),
                id: winlex_core::EntityId {
                    source_id: 2,
                    token: 0x04000001,
                },
            };
            let copies = snapshot.copies();
            assert_eq!(copies[0].1, path);
            assert_eq!(String::from_utf16(&copies[1].1).unwrap(), "2::0x04000001");
            let label = menu_label(&wide("Copy"), &copies[0].1);
            assert!(label.windows(2).any(|pair| pair == [38, 38]));
            assert!(label.contains(&0xD800));
        }
    }
}
