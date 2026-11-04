//! MDI lifecycle, activation projection and one-shot arrangement commands.
use super::content::Content;
use super::metadata::MetadataView;
use super::{document as host, dpi, history, read, write};
use crate::{
    layout::{Arrangement, Placement},
    native::*,
    resources::{self, IDS_PREVIEW, IDS_TAB_CLOSE, IDS_TAB_PIN},
    tabs,
};
use std::rc::Rc;

pub(super) struct Workspace {
    pub mdi: HWND,
    pub tabs: HWND,
    pub documents: Vec<Content>,
    pub icons: crate::icons::DocumentIcons,
    pub history: history::History,
    maximized: bool,
    serial: u64,
}
impl Default for Workspace {
    fn default() -> Self {
        Self {
            mdi: std::ptr::null_mut(),
            tabs: std::ptr::null_mut(),
            documents: vec![],
            icons: crate::icons::DocumentIcons::default(),
            history: history::History::default(),
            maximized: true,
            serial: 0,
        }
    }
}

pub(super) fn active() -> HWND {
    let mdi = read(|a| a.workspace.mdi);
    send(mdi, WM_MDIGETACTIVE, 0, 0) as HWND
}
pub(super) fn show_document(hwnd: HWND) {
    let mdi = read(|a| a.workspace.mdi);
    if unsafe { IsIconic(hwnd) } != 0 {
        send(mdi, WM_MDIRESTORE, hwnd as usize, 0);
    }
    send(mdi, WM_MDIACTIVATE, hwnd as usize, 0);
}
/// Opening from the sidebar shows the document without taking keyboard focus from it.
pub(super) fn keep_focus(open: impl FnOnce()) {
    let (frame, mdi) = read(|a| (a.frame, a.workspace.mdi));
    let previous = unsafe { GetFocus() };
    let sidebar = !previous.is_null()
        && unsafe { IsChild(frame, previous) != 0 && IsChild(mdi, previous) == 0 };
    open();
    // Win32 gives a newly active MDI child the focus even when the request came
    // from beside the MDI client; hand it back.
    if sidebar && unsafe { IsWindow(previous) != 0 } {
        focus(previous);
    }
}
pub(super) fn register_document(doc: Content) {
    write(|a| a.workspace.documents.push(doc));
}
pub(super) fn unregister_document(hwnd: HWND) {
    let removed = write(|a| {
        let docs = &mut a.workspace.documents;
        docs.iter()
            .position(|d| d.hwnd() == hwnd)
            .map(|index| docs.remove(index))
    });
    drop(removed);
}
pub(super) fn close_document(hwnd: HWND) {
    history::closing(|| close_now(hwnd));
}
fn close_now(hwnd: HWND) {
    let mdi = read(|a| a.workspace.mdi);
    if content(hwnd).is_some() {
        let tabs = read(|a| a.workspace.tabs);
        let previous = unsafe { GetFocus() };
        let tab_focus = previous == tabs;
        let next = if active() == hwnd {
            read(|a| {
                let docs = &a.workspace.documents;
                let index = docs.iter().position(|d| d.hwnd() == hwnd)?;
                docs.get(index + 1)
                    .or_else(|| index.checked_sub(1).and_then(|i| docs.get(i)))
                    .map(|d| d.hwnd())
            })
        } else {
            None
        };
        send(mdi, WM_MDIDESTROY, hwnd as usize, 0);
        if let Some(next) = next {
            keep_focus(|| show_document(next));
        }
        if tab_focus {
            if read(|a| a.workspace.documents.is_empty()) {
                super::sidebar::focus();
            } else {
                focus(tabs);
            }
        } else if read(|a| a.workspace.documents.is_empty())
            && (previous == hwnd || unsafe { IsWindow(previous) } == 0)
        {
            super::sidebar::focus();
        }
    }
}
pub(super) fn preview_document() -> Option<HWND> {
    views()
        .into_iter()
        .find(|d| d.preview.get())
        .map(|d| d.hwnd)
}
pub(super) fn pin_document(hwnd: HWND) {
    let changed = view(hwnd).is_some_and(|doc| doc.preview.replace(false));
    if changed {
        refresh_document_tabs();
    }
}
pub(super) fn sync_document_selection() {
    let current = active();
    let tabs = read(|a| a.workspace.tabs);
    let selected = content(current).map(|d| d.id());
    tabs::select(tabs, selected);
    super::frame::refresh_status();
}
pub(super) fn refresh_document_tabs() {
    let tabs = read(|a| a.workspace.tabs);
    let docs = contents();
    let mut ordinals = std::collections::HashMap::new();
    let items = docs
        .iter()
        .map(|content| {
            let (id, hwnd, preview) = (content.id(), content.hwnd(), content.preview());
            let title = content.title();
            let ordinal = ordinals.entry(title.clone()).or_insert(0);
            *ordinal += 1;
            let suffix = if *ordinal > 1 {
                format!(" ({ordinal})")
            } else {
                String::new()
            };
            let title = format!("{title}{suffix}");
            let mut description = content.description();
            if preview {
                description = format!("{} · {description}", resources::text(IDS_PREVIEW));
            }
            tabs::Item {
                id,
                italic: preview,
                title,
                description,
                icon: send(hwnd, WM_GETICON, ICON_SMALL as usize, 0) as HICON,
            }
        })
        .collect();
    tabs::set_items(tabs, items);
    sync_document_selection();
}
pub(super) fn tab_notification(n: &tabs::Notification) {
    let Some(doc) = content_by_id(n.id) else {
        return;
    };
    let hwnd = doc.hwnd();
    match n.header.code {
        tabs::SELECT => keep_focus(|| show_document(hwnd)),
        tabs::CLOSE => close_document(hwnd),
        tabs::ACTIVATE => pin_document(hwnd),
        tabs::REORDER => {
            write(|a| {
                let docs = &mut a.workspace.documents;
                if let Some(index) = docs.iter().position(|d| d.id() == n.id) {
                    let doc = docs.remove(index);
                    docs.insert(n.index.min(docs.len()), doc);
                }
            });
            refresh_document_tabs();
        }
        tabs::CONTEXT => {
            super::frame::report(tab_menu(hwnd, n.point));
        }
        _ => {}
    }
}
fn tab_menu(hwnd: HWND, point: POINT) -> anyhow::Result<()> {
    let menu = Popup::new()?;
    menu.item(1, resources::wide_text(IDS_TAB_CLOSE), true)?;
    if view(hwnd).is_some_and(|d| d.preview.get()) {
        menu.item(2, resources::wide_text(IDS_TAB_PIN), true)?;
    }
    match menu.track(read(|a| a.frame), point) {
        1 => close_document(hwnd),
        2 => pin_document(hwnd),
        _ => {}
    }
    Ok(())
}
pub(super) fn scale_documents() {
    let docs: Vec<_> = contents()
        .into_iter()
        .filter_map(|d| d.placement().map(|p| (d.hwnd(), p)))
        .collect();
    for (hwnd, placement) in docs {
        if unsafe { IsZoomed(hwnd) == 0 && IsIconic(hwnd) == 0 } {
            place(hwnd, placement.at(dpi().window(hwnd)));
        }
    }
}
pub(super) fn capture_placements() {
    let mdi = read(|a| a.workspace.mdi);
    for doc in contents() {
        let hwnd = doc.hwnd();
        if unsafe { IsZoomed(hwnd) != 0 || IsIconic(hwnd) != 0 } {
            continue;
        }
        let p = Placement {
            bounds: bounds(hwnd, mdi),
            dpi: dpi().window(hwnd),
        };
        doc.set_placement(p);
    }
}
pub(super) fn set_arrangement(arrangement: Arrangement) {
    set_maximized(false);
    let mdi = read(|a| a.workspace.mdi);
    let current = active();
    if !current.is_null() && unsafe { IsZoomed(current) } != 0 {
        send(mdi, WM_MDIRESTORE, current as usize, 0);
    }
    if arrangement != Arrangement::Free {
        send(
            mdi,
            WM_MDITILE,
            if arrangement == Arrangement::Columns {
                MDITILE_VERTICAL as usize
            } else {
                MDITILE_HORIZONTAL as usize
            },
            0,
        );
    }
    capture_placements();
}
pub(super) fn set_maximized(maximized: bool) {
    write(|a| a.workspace.maximized = maximized);
}
fn normal_placement(hwnd: HWND) -> anyhow::Result<Placement> {
    let mut placement: WINDOWPLACEMENT = unsafe { std::mem::zeroed() };
    placement.length = std::mem::size_of::<WINDOWPLACEMENT>() as u32;
    if unsafe { GetWindowPlacement(hwnd, &mut placement) } == 0 {
        return Err(error("Read document placement"));
    }
    Ok(Placement {
        bounds: placement.rcNormalPosition,
        dpi: dpi().window(hwnd),
    })
}
pub(super) fn next_id() -> u64 {
    write(|a| {
        a.workspace.serial += 1;
        a.workspace.serial
    })
}
fn create_window(
    title: &str,
    create: Box<dyn FnOnce(HWND) -> anyhow::Result<Content>>,
    context: &str,
) -> anyhow::Result<HWND> {
    let (mdi, maximized) = read(|a| (a.workspace.mdi, a.workspace.maximized));
    let mut init = host::Init {
        create: Some(create),
        error: None,
    };
    // CreateMDIWindowW consumes this pointer synchronously in WM_CREATE.
    let hwnd = unsafe {
        CreateMDIWindowW(
            w!("Winlex.Document"),
            wide(title).as_ptr(),
            WS_VISIBLE | WS_OVERLAPPEDWINDOW | if maximized { WS_MAXIMIZE } else { 0 },
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            mdi,
            instance(),
            &mut init as *mut host::Init as isize,
        )
    };
    if hwnd.is_null() {
        return Err(init.error.unwrap_or_else(|| error(context)));
    }
    let placement = match normal_placement(hwnd) {
        Ok(placement) => placement,
        Err(error) => {
            close_document(hwnd);
            return Err(error);
        }
    };
    let content = content(hwnd).expect("created content");
    content.set_placement(placement);
    content.prepare();
    Ok(hwnd)
}
pub(super) fn create_document(
    title: &str,
    mut content: super::metadata::MetadataView,
) -> anyhow::Result<Rc<MetadataView>> {
    let hwnd = create_window(
        title,
        Box::new(move |hwnd| {
            content.create(hwnd)?;
            Ok(Content::Declaration(Rc::from(content)))
        }),
        "Create MDI document",
    )?;
    let doc = view(hwnd).expect("created document");
    refresh_document_tabs();
    Ok(doc)
}
pub(super) fn view(hwnd: HWND) -> Option<Rc<MetadataView>> {
    content(hwnd)?.declaration()
}
pub(super) fn views() -> Vec<Rc<MetadataView>> {
    contents()
        .into_iter()
        .filter_map(|c| c.declaration())
        .collect()
}
pub(super) fn by_id(id: u64) -> Option<Rc<MetadataView>> {
    content_by_id(id)?.declaration()
}
pub(super) fn contents() -> Vec<Content> {
    read(|a| a.workspace.documents.clone())
}
pub(super) fn content(hwnd: HWND) -> Option<Content> {
    contents().into_iter().find(|d| d.hwnd() == hwnd)
}
pub(super) fn content_by_id(id: u64) -> Option<Content> {
    contents().into_iter().find(|d| d.id() == id)
}
pub(super) fn create_tool(descriptor: &'static super::tools::Descriptor) -> anyhow::Result<HWND> {
    let mut view = super::tools::ToolView::new(descriptor);
    let hwnd = create_window(
        &resources::text(descriptor.title),
        Box::new(move |hwnd| {
            view.create(hwnd)?;
            Ok(Content::Tool(Rc::from(view)))
        }),
        "Create tool window",
    )?;
    refresh_document_tabs();
    Ok(hwnd)
}
