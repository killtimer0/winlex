//! Common MDI content identity and capabilities, independent of tool business.
use super::metadata::MetadataView;
use super::tools::ToolView;
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
    pub fn status(&self) -> String {
        match self {
            Self::Declaration(_) => String::new(),
            Self::Tool(v) => v.status(),
        }
    }
    pub fn id(&self) -> u64 {
        match self {
            Self::Declaration(v) => v.id,
            Self::Tool(v) => v.id(),
        }
    }
    pub fn hwnd(&self) -> HWND {
        match self {
            Self::Declaration(v) => v.hwnd,
            Self::Tool(v) => v.hwnd(),
        }
    }
    pub fn title(&self) -> String {
        match self {
            Self::Declaration(v) => v.title(),
            Self::Tool(v) => text(v.hwnd()),
        }
    }
    pub fn preview(&self) -> bool {
        matches!(self, Self::Declaration(v) if v.preview.get())
    }
    pub fn is_live(&self) -> bool {
        match self {
            Self::Declaration(v) => v.live.get(),
            Self::Tool(v) => v.is_live(),
        }
    }
    pub fn mark_closed(&self) {
        match self {
            Self::Declaration(v) => v.live.set(false),
            Self::Tool(v) => v.mark_closed(),
        }
    }
    pub fn placement(&self) -> Option<Placement> {
        match self {
            Self::Declaration(v) => v.placement.get(),
            Self::Tool(v) => v.placement(),
        }
    }
    pub fn set_placement(&self, placement: Placement) {
        match self {
            Self::Declaration(v) => v.placement.set(Some(placement)),
            Self::Tool(v) => v.set_placement(placement),
        }
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
    pub fn find_key(&self, msg: &MSG) -> bool {
        match self {
            Self::Declaration(v) => v.find_key(msg),
            Self::Tool(v) => v.key(msg),
        }
    }
    pub fn destroy(&self) {
        forward!(self, v, v.destroy())
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
            Self::Tool(v) => v.description(),
        }
    }
}
