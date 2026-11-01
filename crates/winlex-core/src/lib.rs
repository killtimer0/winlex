//! Synchronous, UI-independent access to an immutable collection of WinMD sources.
//! Search results own their data; declaration views borrow their Catalog.
mod api;
mod model;
mod search;
mod source;
pub use model::*;
pub use search::SearchQuery as CompiledQuery;
pub use search::{
    Comparison, EvidenceField, Literal, MatchEvidence, MatchField, MatchQuality, NameMatcher,
    Operator, Predicate, Query, QueryError, QueryErrorKind, SearchMatch,
};
pub use source::{AssemblyIdentity, EntityId, Source, SourceData};
// Low-level adapters for independent metadata verification.
mod layout;
#[doc(hidden)]
pub mod metadata;

pub use anyhow::{Error, Result};
pub use api::{
    Architecture, Catalog, EntityCategory, EntityContext, EntitySummary, InvalidSearchInput,
    Search, SearchMode, SearchOptions, SearchStep, guid, integer,
};
pub use windows_metadata;
#[cfg(test)]
mod test_support;
