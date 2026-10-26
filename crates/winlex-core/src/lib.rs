//! Synchronous, UI-independent access to an immutable collection of WinMD sources.
//! Public query results own their data; no borrowed metadata rows escape Catalog.
mod api;
mod source;
pub use source::{AssemblyIdentity, Source};
// Low-level adapters for independent metadata verification.
mod layout;
#[doc(hidden)]
pub mod metadata;

pub use anyhow::{Error, Result};
pub use api::{Architecture, Catalog, EntitySummary, SearchMode, SearchOptions, SearchPage};
pub use serde_json::Value;
