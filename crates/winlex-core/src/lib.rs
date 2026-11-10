//! Synchronous, UI-independent access to an immutable collection of WinMD sources.
//! Search results own their data; declaration views borrow their Catalog.
mod api;
mod instantiation;
mod model;
pub mod number;
mod search;
mod selector;
mod source;
pub mod tools;
mod type_expression;
mod winrt;
pub use instantiation::DeclarationTarget;
pub use model::*;
pub use search::SearchQuery as CompiledQuery;
pub use search::{
    EvidenceField, Literal, MatchEvidence, MatchField, MatchQuality, Operator, Predicate, Query,
    QueryError, QueryErrorKind, SearchMatch,
};
pub use source::{AssemblyIdentity, EntityId, Source, SourceData};
pub use winrt::{AbiMethod, AbiParameter, InterfaceIdentity};
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
