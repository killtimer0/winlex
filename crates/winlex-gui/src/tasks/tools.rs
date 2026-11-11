//! Tool work and results independent of native document controls.
use std::path::PathBuf;
use winlex_core::tools::{
    com::{Guid, typelib},
    error_code::{self, Failure, QueryResult, Request},
};

pub enum Job {
    Query { revision: u64, request: Request },
    Languages { path: PathBuf },
    TypeLibraries { libraries: Vec<(Guid, u16, u16)> },
}

pub enum Completion {
    Query {
        revision: u64,
        result: anyhow::Result<QueryResult>,
    },
    Languages {
        path: PathBuf,
        result: Result<Vec<u16>, Failure>,
    },
    TypeLibraries {
        results: Vec<TypeLibraryResult>,
    },
}

pub struct TypeLibraryResult {
    pub guid: Guid,
    pub result: Result<typelib::TypeLibrary, typelib::Error>,
}

pub fn run(job: Job) -> Completion {
    match job {
        Job::Query { revision, request } => Completion::Query {
            revision,
            result: error_code::query(&request),
        },
        Job::Languages { path } => Completion::Languages {
            result: error_code::module_languages(&path),
            path,
        },
        Job::TypeLibraries { libraries } => Completion::TypeLibraries {
            results: libraries
                .into_iter()
                .map(|(guid, major, minor)| TypeLibraryResult {
                    guid,
                    result: typelib::load(guid, major, minor, 0),
                })
                .collect(),
        },
    }
}
