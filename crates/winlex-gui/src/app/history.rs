//! Workspace navigation keeps weak snapshot locators and per-visit reading positions.
use super::{
    catalog,
    metadata::{Binding, MetadataView},
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
    catalog: Weak<Catalog>,
    entity: EntityId,
    architecture: Architecture,
    document: u64,
    position: ViewState,
}

impl Entry {
    fn capture(document: &MetadataView) -> Self {
        let binding = document.binding();
        Self {
            catalog: Arc::downgrade(&binding.catalog),
            entity: binding.id,
            architecture: binding.architecture,
            document: document.id,
            position: document.position(),
        }
    }
    fn same_location(&self, other: &Self) -> bool {
        self.document == other.document
            && self.entity == other.entity
            && self.architecture == other.architecture
            && self.catalog.ptr_eq(&other.catalog)
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
            if self.entries[index].catalog.upgrade().is_none() {
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
    workspace::view(workspace::active()).map(|doc| Entry::capture(&doc))
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

pub(super) fn activation(document: &MetadataView, active: bool) {
    if read(|a| a.workspace.history.depth != 0) {
        return;
    }
    let entry = Entry::capture(document);
    write(|a| {
        if active {
            a.workspace.history.visit(entry);
        } else {
            a.workspace.history.save(entry);
        }
    });
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
        let Some(snapshot) = entry.catalog.upgrade() else {
            write(|a| a.workspace.history.remove(index));
            continue;
        };
        let entity = match snapshot.entity(entry.entity) {
            Ok(entity) => entity,
            Err(error) => {
                catalog::set_status(format!("{error:#}"));
                write(|a| a.workspace.history.remove(index));
                continue;
            }
        };
        let binding = Binding {
            catalog: snapshot,
            id: entry.entity,
            architecture: entry.architecture,
            architecture_mask: entity.architecture_mask,
        };
        let original = workspace::by_id(entry.document).filter(|doc| {
            let current = doc.binding();
            current.id == binding.id && Arc::ptr_eq(&current.catalog, &binding.catalog)
        });
        let hwnd = if let Some(doc) = original {
            if doc.binding().architecture != binding.architecture {
                doc.rebind(&entity.name, binding);
            }
            workspace::show_document(doc.hwnd);
            Some(doc.hwnd)
        } else {
            catalog::open_binding(&entity.name, binding, catalog::Open::Preview)
        };
        if let Some(doc) = hwnd.and_then(workspace::view) {
            doc.focus();
            doc.restore_position(entry.position);
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
            catalog: Arc::downgrade(catalog),
            entity: EntityId {
                source_id: 0,
                token,
            },
            architecture: Architecture::X86,
            document: 1,
            position: ViewState::default(),
        }
    }
    fn replay(history: &mut History, direction: Direction) -> Option<Entry> {
        history.prune();
        let index = history.next(direction)?;
        let entry = history.entries[index].clone();
        history.arrived(index, entry.document);
        Some(entry)
    }

    #[test]
    fn previews_share_a_document_but_replay_preserves_forward_until_new_navigation() {
        let catalog = catalog();
        let mut history = History::default();
        for token in 1..=3 {
            history.visit(entry(&catalog, token));
        }
        assert_eq!(
            replay(&mut history, Direction::Back).unwrap().entity.token,
            2
        );
        history.visit(entry(&catalog, 2));
        assert_eq!(
            replay(&mut history, Direction::Forward)
                .unwrap()
                .entity
                .token,
            3
        );
        assert_eq!(
            replay(&mut history, Direction::Back).unwrap().entity.token,
            2
        );
        history.visit(entry(&catalog, 4));
        assert!(replay(&mut history, Direction::Forward).is_none());
        assert_eq!(
            replay(&mut history, Direction::Back).unwrap().entity.token,
            2
        );
        assert_eq!(
            replay(&mut history, Direction::Back).unwrap().entity.token,
            1
        );
        assert!(replay(&mut history, Direction::Back).is_none());
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
        assert_eq!(
            replay(&mut history, Direction::Back).unwrap().entity.token,
            1
        );
        assert!(replay(&mut history, Direction::Back).is_none());
        assert_eq!(
            replay(&mut history, Direction::Forward)
                .unwrap()
                .entity
                .token,
            2
        );
    }

    #[test]
    fn closing_uses_the_previous_visit_before_a_matching_forward_entry() {
        let catalog = catalog();
        let mut history = History::default();
        for token in [1, 2, 3, 4, 1, 5] {
            history.visit(entry(&catalog, token));
        }
        assert_eq!(
            replay(&mut history, Direction::Back).unwrap().entity.token,
            1
        );
        assert_eq!(
            replay(&mut history, Direction::Back).unwrap().entity.token,
            4
        );
        history.after_close(Some(entry(&catalog, 1)));
        assert!(replay(&mut history, Direction::Back).is_none());
        assert_eq!(
            replay(&mut history, Direction::Forward)
                .unwrap()
                .entity
                .token,
            2
        );
    }

    #[test]
    fn duplicate_documents_architectures_closing_and_capacity_preserve_visit_order() {
        let catalog = catalog();
        let first = entry(&catalog, 1);
        let mut duplicate = first.clone();
        duplicate.document = 2;
        let mut x64 = duplicate.clone();
        x64.architecture = Architecture::X64;
        let mut history = History::default();
        for entry in [&first, &duplicate, &x64] {
            history.visit(entry.clone());
        }
        assert_eq!(
            replay(&mut history, Direction::Back).unwrap().architecture,
            Architecture::X86
        );
        assert_eq!(replay(&mut history, Direction::Back).unwrap().document, 1);
        history.after_close(Some(duplicate.clone()));
        assert_eq!(
            replay(&mut history, Direction::Forward)
                .unwrap()
                .architecture,
            Architecture::X64
        );
        history.after_close(None);
        assert_eq!(
            replay(&mut history, Direction::Back).unwrap().architecture,
            Architecture::X64
        );
        for token in 1..=200 {
            history.visit(entry(&catalog, token));
        }
        let mut remaining = Vec::new();
        while let Some(entry) = replay(&mut history, Direction::Back) {
            remaining.push(entry.entity.token);
        }
        assert_eq!(remaining, (101..200).rev().collect::<Vec<_>>());
    }
}
