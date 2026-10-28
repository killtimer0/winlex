//! Metadata operations executed by the application's background executor.
use crate::worker::Ticket;
use anyhow::{Context, bail};
use std::{path::PathBuf, sync::Arc};
use winlex_core::{Architecture, Catalog, EntityId, EntitySummary, SearchOptions, SourceData};

pub enum Job {
    Load {
        paths: Vec<PathBuf>,
        previous: Option<Arc<Catalog>>,
        reload: Vec<PathBuf>,
    },
    Search {
        catalog: Arc<Catalog>,
        query: String,
    },
    Describe {
        document: u64,
        catalog: Arc<Catalog>,
        entity: EntityId,
        arch: Architecture,
    },
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Load,
    Search,
    Document(u64),
}
impl Job {
    pub fn target(&self) -> Target {
        match self {
            Self::Load { .. } => Target::Load,
            Self::Search { .. } => Target::Search,
            Self::Describe { document, .. } => Target::Document(*document),
        }
    }
}
pub enum Output {
    Loaded(Option<Arc<Catalog>>),
    Matches(Vec<EntitySummary>),
    SearchDone,
    Description {
        body: winlex_format::declaration::Rendered,
        details: String,
    },
}
pub struct Completion {
    pub id: u64,
    pub target: Target,
    pub result: Result<Output, String>,
}
pub fn run(job: Job, ticket: &Ticket, publish: &dyn Fn(Completion) -> Result<(), String>) {
    let target = job.target();
    let emit = |result| {
        publish(Completion {
            id: ticket.id,
            target,
            result,
        })
    };
    let result = execute(job, ticket, &emit).map_err(|e| format!("{e:#}"));
    let _ = emit(result);
}
fn execute(
    job: Job,
    ticket: &Ticket,
    publish: &impl Fn(Result<Output, String>) -> Result<(), String>,
) -> anyhow::Result<Output> {
    match job {
        Job::Load {
            paths,
            previous,
            reload,
        } => {
            if paths.is_empty() {
                return Ok(Output::Loaded(None));
            }
            let mut inputs = Vec::new();
            for path in paths {
                if ticket.cancelled() {
                    bail!("cancelled");
                }
                let data = if !reload.contains(&path)
                    && let Some(data) = previous
                        .as_ref()
                        .and_then(|c| c.inputs().iter().find(|s| s.path() == path))
                {
                    data.clone()
                } else {
                    SourceData::read(&path).with_context(|| format!("Read {}", path.display()))?
                };
                inputs.push(data);
            }
            Ok(Output::Loaded(Some(Arc::new(Catalog::from_inputs(
                inputs,
            )?))))
        }
        Job::Search { catalog, query } => {
            let mut batch = Vec::with_capacity(128);
            for item in catalog.search(
                &query,
                SearchOptions {
                    cancel: Some(&ticket.cancel),
                    ..Default::default()
                },
            )? {
                batch.push(item?);
                if batch.len() == 128 {
                    publish(Ok(Output::Matches(std::mem::replace(
                        &mut batch,
                        Vec::with_capacity(128),
                    ))))
                    .map_err(winlex_core::Error::msg)?;
                }
            }
            if !batch.is_empty() {
                publish(Ok(Output::Matches(batch))).map_err(winlex_core::Error::msg)?;
            }
            Ok(Output::SearchDone)
        }
        Job::Describe {
            catalog,
            entity,
            arch,
            ..
        } => {
            let body = winlex_format::declaration::render(&catalog, entity, arch)?;
            let details = format!(
                "{:#}",
                winlex_format::describe(&catalog, entity, Some(arch))?
            );
            Ok(Output::Description { body, details })
        }
    }
}
