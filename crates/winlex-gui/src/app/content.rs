//! Common MDI content identity and capabilities, independent of tool business.
use super::{metadata::MetadataView, tools::ToolView};
use crate::{layout::Placement, native::*};
use std::rc::Rc;

#[derive(Clone)]
pub(super) enum Content {
    Declaration(Rc<MetadataView>),
    Tool(Rc<ToolView>),
}
impl Content {
    pub fn status(&self) -> Option<String> {
        match self {
            Self::Declaration(_) => None,
            Self::Tool(v) => Some(v.content.status()),
        }
    }
    pub fn id(&self) -> u64 {
        match self {
            Self::Declaration(v) => v.id,
            Self::Tool(v) => v.id,
        }
    }
    pub fn hwnd(&self) -> HWND {
        match self {
            Self::Declaration(v) => v.hwnd,
            Self::Tool(v) => v.hwnd,
        }
    }
    pub fn title(&self) -> String {
        match self {
            Self::Declaration(v) => v.title(),
            Self::Tool(v) => v.title(),
        }
    }
    pub fn preview(&self) -> bool {
        matches!(self, Self::Declaration(v) if v.preview.get())
    }
    pub fn placement(&self) -> Option<Placement> {
        match self {
            Self::Declaration(v) => v.placement.get(),
            Self::Tool(v) => v.placement.get(),
        }
    }
    pub fn set_placement(&self, p: Placement) {
        match self {
            Self::Declaration(v) => v.placement.set(Some(p)),
            Self::Tool(v) => v.placement.set(Some(p)),
        }
    }
    pub fn focus(&self) {
        match self {
            Self::Declaration(v) => v.focus(),
            Self::Tool(v) => v.content.focus(),
        }
    }
    pub fn layout(&self) {
        match self {
            Self::Declaration(v) => v.layout(),
            Self::Tool(v) => v.content.layout(),
        }
    }
    pub fn prepare(&self) {
        match self {
            Self::Declaration(v) => v.prepare(),
            Self::Tool(v) => v.prepare(),
        }
    }
    pub fn recolor(&self) {
        match self {
            Self::Declaration(v) => v.recolor(),
            Self::Tool(v) => v.recolor(),
        }
    }
    pub fn command(&self, id: u16, code: u32) -> bool {
        match self {
            Self::Declaration(v) => v.command(id, code),
            Self::Tool(v) => v.content.command(id, code),
        }
    }
    pub fn find_key(&self, msg: &MSG) -> bool {
        match self {
            Self::Declaration(v) => v.find_key(msg),
            Self::Tool(v) => v.content.key(msg),
        }
    }
    pub fn destroy(&self) {
        match self {
            Self::Declaration(v) => {
                v.cancel();
                v.destroy();
            }
            Self::Tool(v) => v.content.destroy(),
        }
    }
    pub fn supports_find(&self) -> bool {
        match self {
            Self::Declaration(_) => true,
            Self::Tool(v) => v.content.supports_find(),
        }
    }
    pub fn activated(&self) {
        if let Self::Tool(v) = self {
            v.activated();
        }
    }
    pub fn description(&self) -> String {
        match self {
            Self::Declaration(v) => {
                let b = v.binding();
                format!(
                    "{} · {:08X}",
                    b.catalog.sources()[b.id.source_id].path.display(),
                    b.id.token
                )
            }
            Self::Tool(v) => crate::resources::text(v.descriptor.description),
        }
    }
}
