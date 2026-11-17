//! Metadata operations executed by the application's background executor.
use crate::{
    results::{self, Items, ResultItem},
    worker::Ticket,
};
use anyhow::{Context, bail};
use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
use winlex_core::{
    Architecture, Catalog, DeclarationTarget, SearchMatch, SearchOptions, SearchStep, SourceData,
};

const SEARCH_BATCH_INTERVAL: Duration = Duration::from_millis(100);

pub mod tools;

pub enum LoadInput {
    Read(PathBuf),
    Reuse(SourceData),
}
impl LoadInput {
    pub fn path(&self) -> &std::path::Path {
        match self {
            Self::Read(path) => path,
            Self::Reuse(data) => data.path(),
        }
    }
}
pub enum Job {
    Load {
        inputs: Arc<[LoadInput]>,
    },
    Search {
        catalog: Arc<Catalog>,
        compiled: Box<winlex_core::CompiledQuery>,
        categories: u32,
    },
    Describe {
        document: u64,
        catalog: Arc<Catalog>,
        target: DeclarationTarget,
        arch: Architecture,
    },
    Tool {
        document: u64,
        job: tools::Job,
    },
}
pub enum Completion {
    Loaded {
        id: u64,
        result: anyhow::Result<Option<Arc<Catalog>>>,
    },
    Matches {
        id: u64,
        items: Items,
    },
    SearchDone {
        id: u64,
        result: anyhow::Result<()>,
    },
    Description {
        id: u64,
        document: u64,
        result: anyhow::Result<crate::rich_text::Content>,
    },
    Tool {
        id: u64,
        document: u64,
        completion: tools::Completion,
    },
}
pub fn run(job: Job, ticket: &Ticket, publish: &dyn Fn(Completion) -> anyhow::Result<()>) {
    if ticket.cancelled() {
        return;
    }
    let id = ticket.id;
    let completion = match job {
        Job::Load { inputs } => Completion::Loaded {
            id,
            result: load(&inputs, ticket),
        },
        Job::Search {
            catalog,
            compiled,
            categories,
        } => {
            let result = (|| {
                let mut search = catalog.search_compiled(
                    *compiled,
                    SearchOptions {
                        cancel: Some(&ticket.cancel),
                        categories: Some(categories),
                        ..Default::default()
                    },
                )?;
                search_batches(
                    ticket,
                    || search.step(),
                    &|items| publish(Completion::Matches { id, items }),
                    Instant::now,
                    |item| results::prepare(&catalog, item),
                )
            })();
            Completion::SearchDone { id, result }
        }
        Job::Describe {
            catalog,
            target,
            arch,
            document,
        } => Completion::Description {
            id,
            document,
            result: winlex_format::declaration::render_target(&catalog, &target, arch)
                .map(Into::into),
        },
        Job::Tool { document, job } => Completion::Tool {
            id,
            document,
            completion: tools::run(job, ticket),
        },
    };
    let _ = publish(completion);
}

fn load(inputs: &[LoadInput], ticket: &Ticket) -> anyhow::Result<Option<Arc<Catalog>>> {
    if inputs.is_empty() {
        return Ok(None);
    }
    let mut loaded = Vec::new();
    for input in inputs {
        if ticket.cancelled() {
            bail!("cancelled");
        }
        let data = match input {
            LoadInput::Reuse(data) => data.clone(),
            LoadInput::Read(path) => {
                SourceData::read(path).with_context(|| format!("Read {}", path.display()))?
            }
        };
        loaded.push(data);
    }
    Ok(Some(Arc::new(Catalog::from_inputs(loaded)?)))
}

fn search_batches(
    ticket: &Ticket,
    mut next: impl FnMut() -> anyhow::Result<SearchStep>,
    publish: &impl Fn(Items) -> anyhow::Result<()>,
    mut now: impl FnMut() -> Instant,
    mut prepare: impl FnMut(SearchMatch) -> anyhow::Result<Box<ResultItem>>,
) -> anyhow::Result<()> {
    let mut batch = Vec::new();
    let mut sent = now();
    let result = loop {
        if ticket.cancelled() {
            bail!("cancelled");
        }
        let step = next();
        if ticket.cancelled() {
            bail!("cancelled");
        }
        match step {
            Ok(SearchStep::Match(item)) => match prepare(item) {
                Ok(item) => batch.push(item),
                Err(error) => break Err(error),
            },
            Ok(SearchStep::Skipped) => {}
            Ok(SearchStep::Done) => break Ok(()),
            Err(error) => break Err(error),
        }
        if !batch.is_empty() && now().duration_since(sent) >= SEARCH_BATCH_INTERVAL {
            publish(std::mem::take(&mut batch))?;
            sent = now();
        }
    };
    if !batch.is_empty() {
        publish(batch)?;
    }
    result
}
