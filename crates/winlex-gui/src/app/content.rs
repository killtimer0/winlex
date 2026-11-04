//! Common MDI content identity and capabilities, independent of tool business.
use super::{metadata::MetadataView, tools::ToolView};
use crate::{layout::Placement, native::*};
use std::rc::Rc;

#[derive(Clone)]
pub(super) enum Content {
    Declaration(Rc<MetadataView>),
    Tool(Rc<ToolView>),
}

macro_rules! forward {
    ($this:ident, $view:ident, $value:expr) => {
        match $this {
            Self::Declaration($view) => $value,
            Self::Tool($view) => $value,
        }
    };
}

impl Content {
    pub fn declaration(&self) -> Option<Rc<MetadataView>> {
        match self {
            Self::Declaration(v) => Some(v.clone()),
            Self::Tool(_) => None,
        }
    }
    pub fn tool(&self) -> Option<Rc<ToolView>> {
        match self {
            Self::Declaration(_) => None,
            Self::Tool(v) => Some(v.clone()),
        }
    }
    pub fn status(&self) -> Option<String> {
        match self {
            Self::Declaration(_) => None,
            Self::Tool(v) => Some(v.status()),
        }
    }
    pub fn id(&self) -> u64 {
        forward!(self, v, v.id)
    }
    pub fn hwnd(&self) -> HWND {
        forward!(self, v, v.hwnd)
    }
    pub fn title(&self) -> String {
        forward!(self, v, v.title())
    }
    pub fn preview(&self) -> bool {
        matches!(self, Self::Declaration(v) if v.preview.get())
    }
    pub fn placement(&self) -> Option<Placement> {
        forward!(self, v, v.placement.get())
    }
    pub fn set_placement(&self, p: Placement) {
        forward!(self, v, v.placement.set(Some(p)))
    }
    pub fn focus(&self) {
        forward!(self, v, v.focus())
    }
    pub fn layout(&self) {
        forward!(self, v, v.layout())
    }
    pub fn prepare(&self) {
        forward!(self, v, v.prepare())
    }
    pub fn recolor(&self) {
        forward!(self, v, v.recolor())
    }
    pub fn command(&self, id: u16, code: u32) -> bool {
        match self {
            Self::Declaration(v) => v.command(id, code),
            Self::Tool(_) => false,
        }
    }
    pub fn find_key(&self, msg: &MSG) -> bool {
        match self {
            Self::Declaration(v) => v.find_key(msg),
            Self::Tool(v) => v.key(msg),
        }
    }
    pub fn destroy(&self) {
        match self {
            Self::Declaration(v) => {
                v.cancel();
                v.destroy();
            }
            Self::Tool(v) => v.destroy(),
        }
    }
    pub fn supports_find(&self) -> bool {
        match self {
            Self::Declaration(_) => true,
            Self::Tool(_) => false,
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
                    b.catalog.sources()[b.entity.id.source_id].path.display(),
                    b.entity.id.token
                )
            }
            Self::Tool(v) => crate::resources::text(v.descriptor.description),
        }
    }
}
