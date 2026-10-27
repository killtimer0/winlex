//! Metadata operations executed by the application's background executor.
use crate::worker::Ticket;
use anyhow::{Context, bail};
use std::{path::PathBuf, sync::Arc};
use winlex_core::{
    Architecture, Catalog, EntityId, EntitySummary, SearchMode, SearchOptions, SourceData,
};

pub enum Job {
    Reopen {
        document: u64,
        previous: Arc<Catalog>,
        current: Arc<Catalog>,
        source: usize,
        entity: EntityId,
        arch: Architecture,
    },
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
    Discover {
        files: Vec<PathBuf>,
        directories: Vec<PathBuf>,
    },
    Inspect {
        document: u64,
        path: PathBuf,
    },
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Load,
    Search,
    Discover,
    Document(u64),
    Reopen(u64),
}
impl Job {
    pub fn target(&self) -> Target {
        match self {
            Self::Load { .. } => Target::Load,
            Self::Search { .. } => Target::Search,
            Self::Discover { .. } => Target::Discover,
            Self::Describe { document, .. } | Self::Inspect { document, .. } => {
                Target::Document(*document)
            }
            Self::Reopen { document, .. } => Target::Reopen(*document),
        }
    }
}
#[derive(Clone)]
pub struct Discovered {
    pub missing: bool,
    pub path: PathBuf,
    pub size: Option<u64>,
    pub error: Option<String>,
}
pub enum Output {
    Reopened {
        catalog: Arc<Catalog>,
        entity: Option<EntitySummary>,
    },
    Loaded(Option<Arc<Catalog>>),
    Matches(Vec<EntitySummary>),
    SearchDone,
    Discovered(Vec<Discovered>),
    Inspected(Arc<Catalog>),
    Description(String),
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
        Job::Reopen {
            previous,
            current,
            source,
            entity,
            arch,
            ..
        } => {
            let old = previous.entity(entity)?;
            let mut matches = current.search(
                &old.full_name,
                SearchOptions {
                    source: Some(source),
                    mode: SearchMode::Exact,
                    case_sensitive: true,
                    architecture: Some(arch),
                    cancel: Some(&ticket.cancel),
                    ..Default::default()
                },
            )?;
            let entity = matches.next().transpose()?;
            if matches.next().transpose()?.is_some() {
                bail!("Current entity is ambiguous");
            }
            drop(matches);
            Ok(Output::Reopened {
                catalog: current,
                entity,
            })
        }
        Job::Load {
            paths,
            previous,
            reload,
        } => {
            if paths.is_empty() {
                return Ok(Output::Loaded(None));
            }
            let mut inputs: Vec<SourceData> = Vec::new();
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
        Job::Discover { files, directories } => {
            let mut paths = files;
            let mut entries = Vec::new();
            for directory in directories {
                if ticket.cancelled() {
                    bail!("cancelled");
                }
                match std::fs::read_dir(&directory) {
                    Ok(iter) => {
                        for entry in iter {
                            match entry {
                                Ok(e) => {
                                    let path = e.path();
                                    if path
                                        .extension()
                                        .is_some_and(|s| s.eq_ignore_ascii_case("winmd"))
                                    {
                                        paths.push(path);
                                    }
                                }
                                Err(e) => entries.push(Discovered {
                                    missing: e.kind() == std::io::ErrorKind::NotFound,
                                    path: directory.clone(),
                                    size: None,
                                    error: Some(e.to_string()),
                                }),
                            }
                        }
                    }
                    Err(e) => entries.push(Discovered {
                        missing: e.kind() == std::io::ErrorKind::NotFound,
                        path: directory,
                        size: None,
                        error: Some(e.to_string()),
                    }),
                }
            }
            paths.sort();
            paths.dedup();
            for path in paths {
                if ticket.cancelled() {
                    bail!("cancelled");
                }
                let (size, error, missing) = match std::fs::metadata(&path) {
                    Ok(data) => (Some(data.len()), None, false),
                    Err(e) => {
                        let missing = e.kind() == std::io::ErrorKind::NotFound;
                        (None, Some(e.to_string()), missing)
                    }
                };
                entries.push(Discovered {
                    missing,
                    path,
                    size,
                    error,
                });
            }
            entries.sort_by(|a, b| a.path.cmp(&b.path));
            Ok(Output::Discovered(entries))
        }
        Job::Inspect { path, .. } => Catalog::open(path).map(|c| Output::Inspected(Arc::new(c))),
        Job::Describe {
            catalog,
            entity,
            arch,
            ..
        } => winlex_format::describe(&catalog, entity, Some(arch))
            .map(|value| Output::Description(format!("{value:#}").replace('\n', "\r\n"))),
    }
}
