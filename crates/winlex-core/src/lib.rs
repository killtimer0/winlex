//! Synchronous, UI-independent access to a single immutable WinMD snapshot.
//! Public query results own their data; no borrowed metadata rows escape Catalog.
mod api;
// Low-level adapters for independent metadata verification.
#[doc(hidden)]
pub mod layout;
#[doc(hidden)]
pub mod metadata;

pub use anyhow::{Error, Result};
pub use api::{Architecture, Catalog, EntitySummary, SearchMode, SearchOptions, SearchPage};
pub use serde_json::Value;
