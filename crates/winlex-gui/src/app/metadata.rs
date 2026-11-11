//! Snapshot-bound declaration documents. Catalog changes update only the version hint.
use super::{bind, catalog, document, history, settings, workspace};
use crate::app::context::Context;
use crate::{
    layout,
    native::*,
    resources::{self, *},
    tasks::Job,
};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::Arc,
};
use winlex_core::{Architecture, Catalog, DeclarationTarget};

mod find;
mod generic;
mod info;

#[derive(Clone)]
pub(super) struct Binding {
    pub catalog: Arc<Catalog>,
    pub entity: Arc<winlex_core::EntitySummary>,
    pub target: DeclarationTarget,
    pub architecture: Architecture,
}
pub(super) enum ArchitectureChoice {
    Exact(Architecture),
    Preferred {
        current: Option<Architecture>,
        preferred: Option<Architecture>,
    },
}
impl Binding {
    pub(super) fn resolve(
        catalog: Arc<Catalog>,
        target: impl Into<DeclarationTarget>,
        choice: ArchitectureChoice,
    ) -> anyhow::Result<Self> {
        let target = target.into();
        let entity = catalog.entity(target.id)?;
        let mask = entity.architecture_mask;
        let architecture = match choice {
            ArchitectureChoice::Exact(arch) => arch.supports(mask).then_some(arch),
            ArchitectureChoice::Preferred { current, preferred } => current
                .filter(|arch| arch.supports(mask))
                .or_else(|| preferred_for(mask, preferred)),
        }
        .ok_or_else(|| anyhow::anyhow!(resources::text(IDS_UNSUPPORTED_ARCH)))?;
        let target = catalog.bind_target(&target, architecture)?;
        let entity = winlex_format::declaration::target_summary(&catalog, &target, architecture)?;
        Ok(Self {
            catalog,
            entity: Arc::new(entity),
            target,
            architecture,
        })
    }
    pub(super) fn same_entity(&self, other: &Self) -> bool {
        self.target == other.target && Arc::ptr_eq(&self.catalog, &other.catalog)
    }
    pub(super) fn same_location(&self, other: &Self) -> bool {
        self.same_entity(other) && self.architecture == other.architecture
    }
}
struct State {
    binding: Binding,
    ticket: Option<u64>,
    restore: Option<crate::rich_text::ViewState>,
    content: Option<Rc<crate::rich_text::Content>>,
    press: Option<LinkPress>,
    navigation: Option<Navigation>,
    hover: Option<std::ops::Range<usize>>,
    tracking: bool,
}
struct Navigation {
    content: Rc<crate::rich_text::Content>,
    binding: Binding,
    target: DeclarationTarget,
}
struct LinkPress {
    navigation: Navigation,
    range: std::ops::Range<usize>,
    origin: POINT,
}
impl State {
    fn new(binding: Binding) -> Self {
        Self {
            binding,
            ticket: None,
            restore: None,
            content: None,
            press: None,
            navigation: None,
            hover: None,
            tracking: false,
        }
    }
    fn clear_interactions(&mut self) {
        self.press = None;
        self.navigation = None;
        self.hover = None;
    }
    fn contains(&self, navigation: &Navigation) -> bool {
        self.content
            .as_ref()
            .is_some_and(|content| Rc::ptr_eq(content, &navigation.content))
            && self.binding.same_location(&navigation.binding)
    }
}
pub(super) struct MetadataView {
    // The document lives inside the context, so the pointer cannot outlive it.
    ctx: *const Context,
    pub id: u64,
    pub hwnd: HWND,
    pub placement: Cell<Option<layout::Placement>>,
    pub preview: Cell<bool>,
    pub live: Cell<bool>,
    state: RefCell<State>,
    icons: RefCell<Option<Rc<crate::icons::WindowIcons>>>,
    editor: HWND,
    header: HWND,
    info: info::Info,
    find: find::Find,
}
pub(super) fn find(ctx: &Context, binding: &Binding) -> Option<HWND> {
    workspace::views(ctx).into_iter().find_map(|content| {
        let matches = content.state.borrow().binding.same_location(binding);
        matches.then_some(content.hwnd)
    })
}
pub(super) fn open(ctx: &Context, binding: Binding, preview: bool) -> anyhow::Result<HWND> {
    let content = MetadataView {
        ctx: ctx as *const Context,
        id: workspace::next_serial(ctx),
        hwnd: std::ptr::null_mut(),
        placement: Cell::new(None),
        preview: Cell::new(preview),
        live: Cell::new(true),
        icons: RefCell::new(None),
        state: RefCell::new(State::new(binding)),
        editor: std::ptr::null_mut(),
        header: std::ptr::null_mut(),
        info: info::Info::default(),
        find: find::Find::default(),
    };
    let title = content.title();
    let content = workspace::create_document(ctx, &title, content)?;
    content.request();
    Ok(content.hwnd)
}
impl MetadataView {
    fn ctx(&self) -> &Context {
        unsafe { &*self.ctx }
    }
    pub(super) fn is_live(&self) -> bool {
        self.live.get()
    }
    pub(super) fn binding(&self) -> Binding {
        self.state.borrow().binding.clone()
    }
    pub(super) fn rebind(&self, binding: Binding) {
        {
            let mut state = self.state.borrow_mut();
            state.binding = binding;
            state.clear_interactions();
        }
        self.fill_architectures();
        self.ctx().checked(self.info.fill(&self.binding()));
        self.ctx().checked(self.update_icons());
        self.request();
    }
    pub(super) fn title(&self) -> String {
        let state = self.state.borrow();
        format!(
            "{} - {}",
            state.binding.entity.name,
            state.binding.architecture.name()
        )
    }
    fn update_icons(&self) -> anyhow::Result<()> {
        let icon = {
            let state = self.state.borrow();
            crate::icons::Icon::for_kind(&state.binding.entity.kind)
        };
        super::document::update_icons(self.ctx(), self.hwnd, icon, &self.icons)
    }
    fn fill_architectures(&self) {
        let binding = self.state.borrow().binding.clone();
        send(self.info.architecture, CB_RESETCONTENT, 0, 0);
        for arch in Architecture::supported(binding.entity.architecture_mask) {
            let index = send(
                self.info.architecture,
                CB_ADDSTRING,
                0,
                wide(arch.name()).as_ptr() as isize,
            );
            if arch == binding.architecture {
                send(self.info.architecture, CB_SETCURSEL, index as usize, 0);
            }
        }
    }
    fn request(&self) {
        if !self.is_live() {
            return;
        }
        let ctx = self.ctx();
        let ticket = catalog::next_ticket(ctx);
        let (binding, previous) = {
            let mut state = self.state.borrow_mut();
            state.restore = None;
            let previous = state.ticket.replace(ticket.id);
            (state.binding.clone(), previous)
        };
        document::update_title(ctx, self.hwnd, &self.title());
        self.layout();
        let job = Job::Describe {
            document: self.id,
            catalog: binding.catalog,
            target: binding.target,
            arch: binding.architecture,
        };
        let body = match ctx.replace_job(previous, ticket, job) {
            Ok(()) => resources::text(IDS_QUERYING),
            Err(error) => {
                self.state.borrow_mut().ticket = None;
                format!("{error:#}")
            }
        };
        self.set_content(crate::rich_text::Content::new(body));
    }
    fn set_content(&self, content: crate::rich_text::Content) {
        let content = Rc::new(content);
        {
            let mut state = self.state.borrow_mut();
            state.clear_interactions();
            state.content = Some(content.clone());
        }
        super::status::report(
            self.ctx(),
            crate::rich_text::set_content(self.editor, &content),
        );
        self.find.refresh(self.editor, false);
    }
    fn select_architecture(&self, code: u32) {
        let binding = self.binding();
        let supported: Vec<_> = Architecture::supported(binding.entity.architecture_mask).collect();
        if code == CBN_SELENDCANCEL {
            if let Some(index) = supported
                .iter()
                .position(|arch| *arch == binding.architecture)
            {
                send(self.info.architecture, CB_SETCURSEL, index, 0);
            }
            return;
        }
        if code != CBN_SELENDOK {
            return;
        }
        let index = send(self.info.architecture, CB_GETCURSEL, 0, 0);
        let Some(&arch) = supported.get(index as usize) else {
            return;
        };
        let Some(resolved) = super::status::report(
            self.ctx(),
            Binding::resolve(
                binding.catalog.clone(),
                binding.target.clone(),
                ArchitectureChoice::Exact(arch),
            ),
        ) else {
            if let Some(index) = supported
                .iter()
                .position(|arch| *arch == binding.architecture)
            {
                send(self.info.architecture, CB_SETCURSEL, index, 0);
            }
            return;
        };
        if supported.len() > 1 {
            super::status::report(
                self.ctx(),
                settings::set_preferred_architecture(self.ctx(), arch),
            );
        }
        if arch != binding.architecture {
            history::visit(self.ctx(), || {
                self.rebind(resolved);
            });
        }
    }
}
impl MetadataView {
    pub(super) fn position(&self) -> crate::rich_text::ViewState {
        let pending = self.state.borrow().restore;
        pending.unwrap_or_else(|| crate::rich_text::ViewState::capture(self.editor))
    }
    pub(super) fn restore_position(&self, position: crate::rich_text::ViewState) {
        let loading = {
            let mut state = self.state.borrow_mut();
            let loading = state.ticket.is_some();
            state.restore = loading.then_some(position);
            loading
        };
        if !loading {
            position.restore(self.editor);
        }
    }
    pub(super) fn cancel_restore(&self) {
        self.state.borrow_mut().restore = None;
    }
    pub(super) fn focus(&self) {
        focus(self.editor);
    }
    pub(super) fn complete(&self, ticket: u64, result: anyhow::Result<crate::rich_text::Content>) {
        let restore = {
            let mut state = self.state.borrow_mut();
            if state.ticket != Some(ticket) {
                return;
            }
            state.ticket = None;
            state.restore.take().filter(|_| result.is_ok())
        };
        let content = match result {
            Ok(content) => content,
            Err(error) => crate::rich_text::Content::new(format!("{error:#}")),
        };
        self.set_content(content);
        if let Some(position) = restore {
            position.restore(self.editor);
        }
    }
    pub(super) fn worker_stopped(&self) {
        self.cancel_restore();
        let pending = self.state.borrow_mut().ticket.take().is_some();
        if pending {
            self.set_content(crate::rich_text::Content::new(resources::text(
                IDS_WORKER_STOPPED,
            )));
        }
    }
    pub(super) fn context_menu(&self, control: HWND, location: isize) -> bool {
        if control != self.editor {
            return false;
        }
        let (content, binding) = {
            let state = self.state.borrow();
            (state.content.clone(), state.binding.clone())
        };
        let regions = content.as_ref().map_or(&[][..], |c| c.regions.as_slice());
        if let Some(Some(target)) = super::status::report(
            self.ctx(),
            crate::rich_text::context_menu(self.editor, location, regions),
        ) {
            self.queue_navigation(Navigation {
                content: content.expect("reference content"),
                binding,
                target,
            });
        }
        true
    }
    pub(super) fn cancel_link_press(&self) {
        self.state.borrow_mut().press = None;
    }
    pub(super) fn move_link_press(&self, point: POINT) {
        let mut state = self.state.borrow_mut();
        if state
            .press
            .as_ref()
            .is_some_and(|press| !click_distance(press.origin, point))
        {
            state.press = None;
        }
    }
    pub(super) fn press_link(&self, point: POINT) {
        self.cancel_link_press();
        if !link_modifier() {
            return;
        }
        let (content, binding) = {
            let state = self.state.borrow();
            (state.content.clone(), state.binding.clone())
        };
        let Some(content) = content else { return };
        let Some((target, range)) = crate::rich_text::region_at_point(self.editor, &content, point)
            .and_then(|region| {
                region
                    .target()
                    .map(|target| (target.clone(), region.range.clone()))
            })
        else {
            return;
        };
        self.state.borrow_mut().press = Some(LinkPress {
            navigation: Navigation {
                content,
                binding,
                target,
            },
            range,
            origin: point,
        });
    }
    pub(super) fn release_link(&self, point: POINT) {
        let Some(press) = self.state.borrow_mut().press.take() else {
            return;
        };
        let same_symbol =
            crate::rich_text::region_at_point(self.editor, &press.navigation.content, point)
                .is_some_and(|region| region.range == press.range);
        if same_symbol && click_distance(press.origin, point) {
            self.queue_navigation(press.navigation);
        }
    }
    /// Update the single Ctrl-hovered symbol; returns whether it is navigable.
    pub(super) fn hover(&self, point: POINT) -> bool {
        let range = link_modifier()
            .then(|| {
                let content = self.state.borrow().content.clone()?;
                crate::rich_text::region_at_point(self.editor, &content, point)
                    .filter(|region| region.target().is_some())
                    .map(|region| region.range.clone())
            })
            .flatten();
        self.set_hover(range.clone());
        range.is_some()
    }
    pub(super) fn hover_cursor(&self) -> Option<bool> {
        let mut point = POINT { x: 0, y: 0 };
        unsafe {
            GetCursorPos(&mut point);
            ScreenToClient(self.editor, &mut point);
        }
        let area = client(self.editor);
        if point.x < 0 || point.y < 0 || point.x >= area.right || point.y >= area.bottom {
            self.set_hover(None);
            return None;
        }
        Some(self.hover(point))
    }
    pub(super) fn mouse_leave(&self) {
        self.state.borrow_mut().tracking = false;
        self.set_hover(None);
    }
    pub(super) fn hand(&self) -> bool {
        self.state.borrow().hover.is_some()
    }
    pub(super) fn clear_hover(&self) {
        self.set_hover(None);
    }
    pub(super) fn track_leave(&self, hwnd: HWND) {
        {
            let mut state = self.state.borrow_mut();
            if state.tracking {
                return;
            }
            state.tracking = true;
        }
        let mut track = TRACKMOUSEEVENT {
            cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
            dwFlags: TME_LEAVE,
            hwndTrack: hwnd,
            dwHoverTime: 0,
        };
        unsafe { TrackMouseEvent(&mut track) };
    }
    fn set_hover(&self, range: Option<std::ops::Range<usize>>) {
        let previous = std::mem::replace(&mut self.state.borrow_mut().hover, range.clone());
        if previous == range {
            return;
        }
        if let Some(previous) = previous {
            let _ = crate::rich_text::set_underline(self.editor, &previous, false);
        }
        if let Some(range) = range {
            let _ = crate::rich_text::set_underline(self.editor, &range, true);
        }
    }
    fn queue_navigation(&self, navigation: Navigation) {
        if !self.is_live() {
            return;
        }
        {
            let mut state = self.state.borrow_mut();
            if !state.contains(&navigation) {
                return;
            }
            state.navigation = Some(navigation);
        }
        if unsafe {
            PostMessageW(
                self.hwnd,
                document::FOLLOW_REFERENCE,
                self.id as u32 as usize,
                (self.id >> 32) as isize,
            )
        } == 0
        {
            self.state.borrow_mut().navigation = None;
            super::status::show_notice(
                self.ctx(),
                format!("{:#}", error("Queue declaration navigation")),
            );
        }
    }
    pub(super) fn follow_reference(&self) {
        let navigation = {
            let mut state = self.state.borrow_mut();
            state
                .navigation
                .take()
                .filter(|request| state.contains(request))
        };
        let Some(navigation) = navigation else { return };
        self.open_target(navigation.binding, navigation.target);
    }
    pub(super) fn create(&mut self, hwnd: HWND) -> anyhow::Result<()> {
        self.hwnd = hwnd;
        self.editor = crate::rich_text::create(hwnd, DECLARATION)?;
        self.header = unsafe {
            child(
                hwnd,
                w!("STATIC"),
                resources::wide_text(IDS_OLD_VERSION),
                0,
                DOCUMENT_HEADER,
            )
        }?;
        document::track_editor(hwnd, self.editor)?;
        show(self.header, false);
        self.info.create(hwnd, self.editor)?;
        self.find.create(hwnd)?;
        use windows::Win32::UI::Controls::RichEdit::{
            EM_GETEVENTMASK, EM_SETEVENTMASK, ENM_SELCHANGE,
        };
        let mask = send(self.editor, EM_GETEVENTMASK, 0, 0);
        send(
            self.editor,
            EM_SETEVENTMASK,
            0,
            mask | ENM_SELCHANGE as isize,
        );
        self.fill_architectures();
        let binding = self.state.borrow().binding.clone();
        self.info.fill(&binding)?;
        self.update_icons()?;
        Ok(())
    }
    pub(super) fn prepare(&self) {
        let hwnd = self.hwnd;
        let ctx = self.ctx();
        ctx.checked(self.update_icons());
        let (font, _) = ctx.font_for(ctx.dpi().window(hwnd));
        bind(self.header, font);
        self.find.prepare(font);
        ctx.checked(self.info.prepare(&ctx.metrics(hwnd)));
        let (system_font, _) = ctx.font_for(ctx.dpi().system());
        ctx.checked(crate::rich_text::prepare(
            self.editor,
            system_font,
            ctx.dpi(),
        ));
        self.layout();
    }
    pub(super) fn recolor(&self) {
        self.ctx().checked(self.update_icons());
        self.cancel_link_press();
        self.state.borrow_mut().hover = None;
        let content = self.state.borrow().content.clone();
        if let Some(content) = content {
            super::status::report(self.ctx(), crate::rich_text::recolor(self.editor, &content));
        }
    }
    pub(super) fn layout(&self) {
        let hwnd = self.hwnd;
        let old = old_version(self.ctx(), &self.state.borrow().binding);
        show(self.header, old);
        let m = self.ctx().metrics(hwnd);
        let mut area = layout::inset(client(hwnd), m.px(8));
        let mut positions = Vec::new();
        if old {
            positions.push((self.header, layout::take_top(&mut area, m.height + m.px(8))));
        }
        self.info.layout(&mut area, &m, &mut positions);
        self.find.layout(self.ctx(), &mut area, &m, &mut positions);
        positions.push((self.editor, area));
        self.ctx().checked(place_all(&positions));
    }
    pub(super) fn command(&self, id: u16, code: u32) -> bool {
        let visible = self.find.visible();
        if self.find.command(self.editor, id, code) {
            if self.find.visible() != visible {
                self.layout();
            }
            return true;
        }
        match id {
            SELECT_ARCH => self.select_architecture(code),
            DOCUMENT_GENERIC if code == BN_CLICKED => {
                let binding = self.binding();
                if let Some(Some(target)) = super::status::report(
                    self.ctx(),
                    generic::show(self.ctx(), self.hwnd, &binding),
                ) && self.is_live()
                    && self.binding().same_location(&binding)
                {
                    self.open_target(binding, target);
                }
            }
            DOCUMENT_MORE if code == BN_CLICKED => {
                let binding = self.binding();
                let open_definition = super::status::report(
                    self.ctx(),
                    self.info.menu(self.ctx(), self.hwnd, &binding),
                );
                if open_definition == Some(true)
                    && self.is_live()
                    && self.binding().same_location(&binding)
                {
                    let target = binding.target.id.into();
                    self.open_target(binding, target);
                }
            }
            DOCUMENT_ONLINE if code == BN_CLICKED => {
                super::status::report(
                    self.ctx(),
                    self.info
                        .open_online(self.hwnd, settings::preferences(self.ctx()).search_engine),
                );
            }
            _ => return false,
        }
        true
    }
    pub(super) fn color(&self, control: HWND, dc: HDC) -> Option<isize> {
        self.info.color(control, dc)
    }
    pub(super) fn destroy(&self) {
        if let Some(ticket) = &self.state.borrow().ticket {
            self.ctx().cancel_job(*ticket);
        }
        self.info.clear_accessibility();
    }
    pub(super) fn find_key(&self, msg: &MSG) -> bool {
        self.find
            .key(self.editor, msg)
            .is_some_and(|id| self.command(id, 0))
    }
    pub(super) fn selection_changed(&self) {
        self.find.count(self.editor, false);
    }

    fn open_target(&self, binding: Binding, target: DeclarationTarget) {
        let Some(definition) = super::status::report(
            self.ctx(),
            Binding::resolve(
                binding.catalog,
                target,
                ArchitectureChoice::Exact(binding.architecture),
            ),
        ) else {
            return;
        };
        history::visit(self.ctx(), || {
            if let Some(hwnd) =
                catalog::open_binding(self.ctx(), definition, catalog::Open::Preview, None)
                && let Some(document) = workspace::view(self.ctx(), hwnd)
            {
                document.focus();
            }
        });
    }
}
pub(super) fn link_modifier() -> bool {
    unsafe { GetKeyState(VK_CONTROL as i32) < 0 && GetKeyState(VK_MENU as i32) >= 0 }
}
fn click_distance(origin: POINT, point: POINT) -> bool {
    (point.x - origin.x).abs() < unsafe { GetSystemMetrics(SM_CXDRAG) }.max(1)
        && (point.y - origin.y).abs() < unsafe { GetSystemMetrics(SM_CYDRAG) }.max(1)
}
fn preferred_for(mask: i32, preferred: Option<Architecture>) -> Option<Architecture> {
    preferred
        .filter(|arch| arch.supports(mask))
        .or_else(|| Architecture::supported(mask).next())
}
fn old_version(ctx: &Context, binding: &Binding) -> bool {
    let path = &binding.catalog.sources()[binding.entity.id.source_id].path;
    catalog::current_catalog(ctx).is_some_and(|current| {
        !Arc::ptr_eq(&current, &binding.catalog)
            && current.sources().iter().any(|source| &source.path == path)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::test_support::catalog;

    #[test]
    fn preference_falls_back_to_canonical_order() {
        assert_eq!(preferred_for(7, None), Some(Architecture::X86));
        assert_eq!(
            preferred_for(7, Some(Architecture::X64)),
            Some(Architecture::X64)
        );
        assert_eq!(
            preferred_for(7, Some(Architecture::Arm64)),
            Some(Architecture::Arm64)
        );
        assert_eq!(
            preferred_for(3, Some(Architecture::Arm64)),
            Some(Architecture::X86)
        );
        assert_eq!(
            preferred_for(4, Some(Architecture::X64)),
            Some(Architecture::Arm64)
        );
        assert_eq!(preferred_for(1, None), Some(Architecture::X86));
        assert_eq!(preferred_for(2, None), Some(Architecture::X64));
        assert_eq!(preferred_for(0, None), None);
    }

    #[test]
    fn materialization_keeps_snapshot_identity_and_exact_architecture() {
        let snapshot = catalog();
        let entity = crate::test_support::search_results(
            snapshot.search("SECURITY_ATTRIBUTES", Default::default()),
        )
        .find(|entity| entity.name == "SECURITY_ATTRIBUTES")
        .unwrap();
        let first = Binding::resolve(
            snapshot.clone(),
            entity.id,
            ArchitectureChoice::Exact(Architecture::X86),
        )
        .unwrap();
        assert_eq!(first.entity.name, entity.name);
        assert_eq!(first.entity.architecture_mask, entity.architecture_mask);
        assert_eq!(first.architecture, Architecture::X86);
        assert!(Arc::ptr_eq(&first.catalog, &snapshot));
        let other_architecture = Binding::resolve(
            snapshot,
            entity.id,
            ArchitectureChoice::Exact(Architecture::X64),
        )
        .unwrap();
        assert_eq!(other_architecture.architecture, Architecture::X64);
        assert!(first.same_entity(&other_architecture));
        assert!(!first.same_location(&other_architecture));
        let reloaded = Binding::resolve(
            catalog(),
            entity.id,
            ArchitectureChoice::Exact(Architecture::X86),
        )
        .unwrap();
        assert!(!first.same_entity(&reloaded));
    }
}
