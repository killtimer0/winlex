//! Metadata operations executed by the application's background executor.
use crate::worker::Ticket;
use anyhow::{Context, bail};
use std::{path::PathBuf, sync::Arc};
use winlex_core::{Architecture, Catalog, EntitySummary, SourceData};

pub enum Job {
    Reopen {
        document: u64,
        previous: Arc<Catalog>,
        current: Arc<Catalog>,
        source: String,
        entity: String,
        arch: Architecture,
    },
    Load {
        paths: Vec<PathBuf>,
        previous: Vec<SourceData>,
        reload: Vec<PathBuf>,
    },
    Search {
        catalog: Arc<Catalog>,
        query: String,
    },
    Describe {
        document: u64,
        catalog: Arc<Catalog>,
        entity: String,
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
    pub id: Option<String>,
    pub path: PathBuf,
    pub size: Option<u64>,
    pub error: Option<String>,
}
pub struct Snapshot {
    pub catalog: Option<Arc<Catalog>>,
    pub inputs: Vec<SourceData>,
}
pub enum Output {
    Reopened {
        catalog: Arc<Catalog>,
        entity: Option<EntitySummary>,
    },
    Loaded(Snapshot),
    Matches(Vec<EntitySummary>),
    SearchDone(usize),
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
            let old = previous.describe(&entity, Some(arch))?;
            let name = old["entity"]["full_name"]
                .as_str()
                .context("Missing full name")?;
            let target = current
                .sources()
                .iter()
                .find(|s| s.id == source)
                .context("Current source unavailable")?;
            let mut matches = Vec::new();
            current.search_batches(name, 128, Some(&ticket.cancel), |batch| {
                let mask = match arch {
                    Architecture::X86 => 1,
                    Architecture::X64 => 2,
                    Architecture::Arm64 => 4,
                };
                matches.extend(batch.into_iter().filter(|e| {
                    e.full_name == name
                        && e.source_id == target.id
                        && e.architecture_mask & mask != 0
                }));
                Ok(())
            })?;
            let entity = match matches.len() {
                0 => None,
                1 => matches.pop(),
                _ => bail!("Current entity is ambiguous"),
            };
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
                return Ok(Output::Loaded(Snapshot {
                    catalog: None,
                    inputs: vec![],
                }));
            }
            let mut inputs: Vec<SourceData> = Vec::new();
            for path in paths {
                if ticket.cancelled() {
                    bail!("cancelled");
                }
                let data = if !reload.contains(&path)
                    && let Some(data) = previous.iter().find(|s| s.path() == path)
                {
                    data.clone()
                } else {
                    SourceData::read(&path).with_context(|| format!("Read {}", path.display()))?
                };
                let data = inputs
                    .iter()
                    .find(|s| s.id() == data.id())
                    .map(|s| s.with_path(path))
                    .unwrap_or(data);
                inputs.push(data);
            }
            let catalog = Arc::new(Catalog::from_inputs(inputs.clone())?);
            Ok(Output::Loaded(Snapshot {
                catalog: Some(catalog),
                inputs,
            }))
        }
        Job::Search { catalog, query } => catalog
            .search_batches(&query, 128, Some(&ticket.cancel), |batch| {
                publish(Ok(Output::Matches(batch))).map_err(winlex_core::Error::msg)
            })
            .map(Output::SearchDone),
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
                                        && !paths.contains(&path)
                                    {
                                        paths.push(path);
                                    }
                                }
                                Err(e) => entries.push(Discovered {
                                    missing: e.kind() == std::io::ErrorKind::NotFound,
                                    id: None,
                                    path: directory.clone(),
                                    size: None,
                                    error: Some(e.to_string()),
                                }),
                            }
                        }
                    }
                    Err(e) => entries.push(Discovered {
                        missing: e.kind() == std::io::ErrorKind::NotFound,
                        id: None,
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
                let (size, id, error, missing) = match SourceData::read(&path) {
                    Ok(data) => (
                        Some(data.len() as u64),
                        Some(data.id().to_owned()),
                        None,
                        false,
                    ),
                    Err(e) => {
                        let missing = e
                            .downcast_ref::<std::io::Error>()
                            .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound);
                        (None, None, Some(e.to_string()), missing)
                    }
                };
                entries.push(Discovered {
                    missing,
                    path,
                    size,
                    id,
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
        } => catalog
            .describe(&entity, Some(arch))
            .map(|value| Output::Description(format!("{value:#}").replace('\n', "\r\n"))),
    }
}
