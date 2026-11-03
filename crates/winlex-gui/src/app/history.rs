//! Workspace navigation keeps weak snapshot locators and per-visit reading positions.
use super::{
    catalog,
    metadata::{ArchitectureChoice, Binding, MetadataView},
    read, workspace, write,
};
use crate::{native::*, resources::*, rich_text::ViewState};
use std::sync::{Arc, Weak};
use winlex_core::{Architecture, Catalog, EntityId};

const CAPACITY: usize = 100;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Direction {
    Back,
    Forward,
}

impl Direction {
    pub const ALL: [Self; 2] = [Self::Back, Self::Forward];
    pub fn command(self) -> u16 {
        match self {
            Self::Back => HISTORY_BACK,
            Self::Forward => HISTORY_FORWARD,
        }
    }
    pub fn from_command(id: u16) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|direction| direction.command() == id)
    }
}

#[derive(Clone)]
struct Entry {
    document: u64,
    target: Target,
}
#[derive(Clone)]
enum Target {
    Declaration(DeclarationTarget),
    Live(std::rc::Weak<()>),
}
#[derive(Clone)]
struct DeclarationTarget {
    catalog: Weak<Catalog>,
    entity: EntityId,
    architecture: Architecture,
    position: ViewState,
}

impl Entry {
    fn capture(document: &MetadataView) -> Self {
        let binding = document.binding();
        Self {
            document: document.id,
            target: Target::Declaration(DeclarationTarget {
                catalog: Arc::downgrade(&binding.catalog),
                entity: binding.id,
                architecture: binding.architecture,
                position: document.position(),
            }),
        }
    }
    fn same_location(&self, other: &Self) -> bool {
        self.document == other.document
            && match (&self.target, &other.target) {
                (Target::Declaration(a), Target::Declaration(b)) => {
                    a.entity == b.entity
                        && a.architecture == b.architecture
                        && a.catalog.ptr_eq(&b.catalog)
                }
                (Target::Live(a), Target::Live(b)) => a.ptr_eq(b),
                _ => false,
            }
    }
    fn valid(&self) -> bool {
        match &self.target {
            Target::Declaration(v) => v.catalog.upgrade().is_some(),
            Target::Live(v) => v.upgrade().is_some(),
        }
    }
}

#[derive(Default)]
pub(super) struct History {
    entries: Vec<Entry>,
    cursor: Option<usize>,
    depth: usize,
    enabled: Option<[bool; 2]>,
}

impl History {
    fn remove(&mut self, index: usize) {
        self.entries.remove(index);
        if let Some(cursor) = self.cursor
            && index <= cursor
        {
            self.cursor = cursor.checked_sub(1);
        }
    }
    fn prune(&mut self) {
        for index in (0..self.entries.len()).rev() {
            if !self.entries[index].valid() {
                self.remove(index);
            }
        }
    }
    fn next(&self, direction: Direction) -> Option<usize> {
        let index = match (direction, self.cursor) {
            (Direction::Back, Some(cursor)) => cursor.checked_sub(1)?,
            (Direction::Back, None) => self.entries.len().checked_sub(1)?,
            (Direction::Forward, Some(cursor)) => cursor + 1,
            (Direction::Forward, None) => return None,
        };
        (index < self.entries.len()).then_some(index)
    }
    fn save(&mut self, entry: Entry) {
        if let Some(current) = self.cursor.and_then(|i| self.entries.get_mut(i))
            && current.same_location(&entry)
        {
            *current = entry;
        }
    }
    fn visit(&mut self, entry: Entry) {
        if self
            .cursor
            .is_some_and(|i| self.entries[i].same_location(&entry))
        {
            self.save(entry);
            return;
        }
        self.entries
            .truncate(self.cursor.map_or(self.entries.len(), |i| i + 1));
        self.entries.push(entry);
        if self.entries.len() > CAPACITY {
            self.entries.remove(0);
        }
        self.cursor = Some(self.entries.len() - 1);
    }
    fn after_close(&mut self, entry: Option<Entry>) {
        let Some(entry) = entry else {
            self.cursor = None;
            return;
        };
        let origin = self.cursor.unwrap_or(self.entries.len());
        let previous = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, candidate)| candidate.same_location(&entry))
            .min_by_key(|(i, _)| (*i > origin, i.abs_diff(origin)))
            .map(|(i, _)| i);
        if let Some(index) = previous {
            self.cursor = Some(index);
            self.save(entry);
        } else {
            self.visit(entry);
        }
    }
    fn arrived(&mut self, index: usize, document: u64) {
        self.cursor = Some(index);
        self.entries[index].document = document;
    }
}

fn active_entry() -> Option<Entry> {
    workspace::content(workspace::active()).map(|doc| capture_content(&doc))
}
fn capture_content(content: &super::content::Content) -> Entry {
    match content {
        super::content::Content::Declaration(v) => Entry::capture(v),
        super::content::Content::Tool(v) => Entry {
            document: v.id,
            target: Target::Live(std::rc::Rc::downgrade(&v.lifetime)),
        },
    }
}
pub(super) fn content_activation(content: &super::content::Content, active: bool) {
    if active {
        content.activated();
    }
    if read(|a| a.workspace.history.depth != 0) {
        return;
    }
    let entry = capture_content(content);
    write(|a| {
        if active {
            a.workspace.history.visit(entry)
        } else {
            a.workspace.history.save(entry)
        }
    });
}

struct Scope {
    outer: bool,
}
impl Scope {
    fn begin() -> Self {
        let outer = write(|a| {
            let history = &mut a.workspace.history;
            history.depth += 1;
            history.depth == 1
        });
        if outer && let Some(entry) = active_entry() {
            write(|a| a.workspace.history.visit(entry));
        }
        Self { outer }
    }
}
impl Drop for Scope {
    fn drop(&mut self) {
        write(|a| a.workspace.history.depth -= 1);
    }
}

pub(super) fn visit<T>(action: impl FnOnce() -> T) -> T {
    let scope = Scope::begin();
    let result = action();
    if scope.outer
        && let Some(entry) = active_entry()
    {
        write(|a| a.workspace.history.visit(entry));
    }
    drop(scope);
    refresh();
    result
}

pub(super) fn closing(action: impl FnOnce()) {
    let scope = Scope::begin();
    action();
    if scope.outer {
        let entry = active_entry();
        write(|a| a.workspace.history.after_close(entry));
    }
    drop(scope);
    refresh();
}

pub(super) fn navigate(direction: Direction) {
    if read(|a| a.workspace.history.depth != 0) {
        return;
    }
    let scope = Scope::begin();
    loop {
        let target = write(|a| {
            let history = &mut a.workspace.history;
            history.prune();
            let index = history.next(direction)?;
            Some((index, history.entries[index].clone()))
        });
        let Some((index, entry)) = target else { break };
        if matches!(entry.target, Target::Live(_)) {
            if let Some(content) = workspace::content_by_id(entry.document) {
                workspace::show_document(content.hwnd());
                content.focus();
                write(|a| a.workspace.history.arrived(index, content.id()));
                break;
            }
            write(|a| a.workspace.history.remove(index));
            continue;
        }
        let Target::Declaration(target) = entry.target else {
            unreachable!()
        };
        let Some(snapshot) = target.catalog.upgrade() else {
            write(|a| a.workspace.history.remove(index));
            continue;
        };
        let Some(binding) = super::frame::report(Binding::resolve(
            snapshot,
            target.entity,
            ArchitectureChoice::Exact(target.architecture),
        )) else {
            write(|a| a.workspace.history.remove(index));
            continue;
        };
        let hwnd = catalog::open_binding(binding, catalog::Open::Preview, Some(entry.document));
        if let Some(doc) = hwnd.and_then(workspace::view) {
            doc.focus();
            doc.restore_position(target.position);
            write(|a| a.workspace.history.arrived(index, doc.id));
        }
        break;
    }
    drop(scope);
    refresh();
}

pub(super) fn refresh() {
    let update = write(|a| {
        let history = &mut a.workspace.history;
        if history.depth != 0 || a.frame.is_null() {
            return None;
        }
        history.prune();
        let enabled = Direction::ALL.map(|direction| history.next(direction).is_some());
        if history.enabled == Some(enabled) {
            return None;
        }
        history.enabled = Some(enabled);
        Some((a.frame, enabled))
    });
    if let Some((frame, enabled)) = update {
        for (direction, enabled) in Direction::ALL.into_iter().zip(enabled) {
            unsafe {
                EnableMenuItem(
                    GetMenu(frame),
                    direction.command() as u32,
                    MF_BYCOMMAND | if enabled { MF_ENABLED } else { MF_GRAYED },
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Direction::{Back, Forward};
    fn declaration(entry: &Entry) -> &DeclarationTarget {
        let Target::Declaration(target) = &entry.target else {
            panic!("expected declaration")
        };
        target
    }

    #[test]
    fn live_history_expires_and_new_instances_do_not_revive_old_visits() {
        let first = std::rc::Rc::new(());
        let second = std::rc::Rc::new(());
        let mut history = History::default();
        let live = |document, life: &std::rc::Rc<()>| Entry {
            document,
            target: Target::Live(std::rc::Rc::downgrade(life)),
        };
        history.visit(live(1, &first));
        history.visit(live(2, &second));
        history.visit(live(1, &first));
        assert_eq!(replay(&mut history, Back).unwrap().document, 2);
        assert_eq!(replay(&mut history, Forward).unwrap().document, 1);
        drop(first);
        history.after_close(Some(live(2, &second)));
        history.prune();
        assert!(replay(&mut history, Back).is_none());
        let fresh = std::rc::Rc::new(());
        history.visit(live(3, &fresh));
        assert_eq!(replay(&mut history, Back).unwrap().document, 2);
        assert_eq!(replay(&mut history, Forward).unwrap().document, 3);
        drop(fresh);
        drop(second);
        history.prune();
        assert!(history.next(Back).is_none());
        assert!(history.next(Forward).is_none());
    }

    fn catalog() -> Arc<Catalog> {
        Arc::new(
            Catalog::open(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../inputs/Windows.Win32.winmd"),
            )
            .unwrap(),
        )
    }
    fn entry(catalog: &Arc<Catalog>, token: u32) -> Entry {
        Entry {
            document: 1,
            target: Target::Declaration(DeclarationTarget {
                catalog: Arc::downgrade(catalog),
                entity: EntityId {
                    source_id: 0,
                    token,
                },
                architecture: Architecture::X86,
                position: ViewState::default(),
            }),
        }
    }
    fn replay(history: &mut History, direction: Direction) -> Option<Entry> {
        history.prune();
        let index = history.next(direction)?;
        let entry = history.entries[index].clone();
        history.arrived(index, entry.document);
        Some(entry)
    }
    #[track_caller]
    fn assert_token(history: &mut History, direction: Direction, expected: Option<u32>) {
        assert_eq!(
            replay(history, direction).map(|entry| declaration(&entry).entity.token),
            expected
        );
    }

    #[test]
    fn previews_share_a_document_but_replay_preserves_forward_until_new_navigation() {
        let catalog = catalog();
        let mut history = History::default();
        for token in 1..=3 {
            history.visit(entry(&catalog, token));
        }
        assert_token(&mut history, Back, Some(2));
        history.visit(entry(&catalog, 2));
        assert_token(&mut history, Forward, Some(3));
        assert_token(&mut history, Back, Some(2));
        history.visit(entry(&catalog, 4));
        assert_token(&mut history, Forward, None);
        assert_token(&mut history, Back, Some(2));
        assert_token(&mut history, Back, Some(1));
        assert_token(&mut history, Back, None);
        for direction in Direction::ALL {
            assert_eq!(
                Direction::from_command(direction.command()),
                Some(direction)
            );
        }
        assert_eq!(Direction::from_command(0), None);
    }

    #[test]
    fn weak_snapshots_expire_and_do_not_alias_the_same_entity_in_a_new_catalog() {
        let old = catalog();
        let current = catalog();
        let weak = Arc::downgrade(&old);
        let mut history = History::default();
        history.visit(entry(&current, 1));
        history.visit(entry(&old, 1));
        history.visit(entry(&current, 2));
        drop(old);
        assert!(weak.upgrade().is_none());
        assert_token(&mut history, Back, Some(1));
        assert_token(&mut history, Back, None);
        assert_token(&mut history, Forward, Some(2));
    }

    #[test]
    fn closing_uses_the_previous_visit_before_a_matching_forward_entry() {
        let catalog = catalog();
        let mut history = History::default();
        for token in [1, 2, 3, 4, 1, 5] {
            history.visit(entry(&catalog, token));
        }
        assert_token(&mut history, Back, Some(1));
        assert_token(&mut history, Back, Some(4));
        history.after_close(Some(entry(&catalog, 1)));
        assert_token(&mut history, Back, None);
        assert_token(&mut history, Forward, Some(2));
    }

    #[test]
    fn duplicate_documents_architectures_closing_and_capacity_preserve_visit_order() {
        let catalog = catalog();
        let first = entry(&catalog, 1);
        let mut duplicate = first.clone();
        duplicate.document = 2;
        let mut x64 = duplicate.clone();
        let Target::Declaration(target) = &mut x64.target else {
            unreachable!()
        };
        target.architecture = Architecture::X64;
        let mut history = History::default();
        for entry in [&first, &duplicate, &x64] {
            history.visit(entry.clone());
        }
        assert_eq!(
            declaration(&replay(&mut history, Back).unwrap()).architecture,
            Architecture::X86
        );
        assert_eq!(replay(&mut history, Back).unwrap().document, 1);
        history.after_close(Some(duplicate.clone()));
        assert_eq!(
            declaration(&replay(&mut history, Forward).unwrap()).architecture,
            Architecture::X64
        );
        history.after_close(None);
        assert_eq!(
            declaration(&replay(&mut history, Back).unwrap()).architecture,
            Architecture::X64
        );
        for token in 1..=200 {
            history.visit(entry(&catalog, token));
        }
        let mut remaining = Vec::new();
        while let Some(entry) = replay(&mut history, Back) {
            remaining.push(declaration(&entry).entity.token);
        }
        assert_eq!(remaining, (101..200).rev().collect::<Vec<_>>());
    }
}
