//! Tool work and results independent of native document controls.
use std::path::PathBuf;
use winlex_core::tools::error_code::{self, Failure, QueryResult, Request};

pub enum Job {
    Query { revision: u64, request: Request },
    Languages { path: PathBuf },
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
    }
}
