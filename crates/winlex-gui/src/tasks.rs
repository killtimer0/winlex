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
        categories: u32,
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
    },
}
pub struct Completion {
    pub id: u64,
    pub target: Target,
    pub result: anyhow::Result<Output>,
}
pub fn run(job: Job, ticket: &Ticket, publish: &dyn Fn(Completion) -> anyhow::Result<()>) {
    let target = job.target();
    let emit = |result| {
        publish(Completion {
            id: ticket.id,
            target,
            result,
        })
    };
    let result = execute(job, ticket, &emit);
    let _ = emit(result);
}
fn execute(
    job: Job,
    ticket: &Ticket,
    publish: &impl Fn(anyhow::Result<Output>) -> anyhow::Result<()>,
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
        Job::Search {
            catalog,
            query,
            categories,
        } => {
            let mut batch = Vec::with_capacity(128);
            for item in catalog.search(
                &query,
                SearchOptions {
                    cancel: Some(&ticket.cancel),
                    categories: Some(categories),
                    ..Default::default()
                },
            )? {
                batch.push(item?);
                if batch.len() == 128 {
                    publish(Ok(Output::Matches(std::mem::replace(
                        &mut batch,
                        Vec::with_capacity(128),
                    ))))?;
                }
            }
            if !batch.is_empty() {
                publish(Ok(Output::Matches(batch)))?;
            }
            Ok(Output::SearchDone)
        }
        Job::Describe {
            catalog,
            entity,
            arch,
            ..
        } => {
            let mut body = winlex_format::declaration::render(&catalog, entity, arch)?;
            let summary = catalog.entity(entity)?;
            let prefix = format!(
                "{} · {}\n{}\n{}\n\n",
                summary.kind,
                summary.full_name,
                catalog.sources()[entity.source_id].path.display(),
                entity
            )
            .replace("\r\n", "\n");
            let offset = prefix.encode_utf16().count();
            for number in &mut body.numbers {
                number.range.start += offset;
                number.range.end += offset;
            }
            let details = format!(
                "{:#}",
                winlex_format::describe(&catalog, entity, Some(arch))?
            );
            body.text = format!("{prefix}{}\n\n{details}", body.text);
            Ok(Output::Description { body })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_error_retains_io_source_across_worker_completion() {
        let worker = crate::worker::Worker::new(run).unwrap();
        let path =
            std::env::temp_dir().join(format!("Winlex.Test.Missing.{}.winmd", std::process::id()));
        assert!(!path.exists());
        worker
            .replace(
                None,
                Ticket::new(1),
                Job::Load {
                    paths: vec![path.clone()],
                    previous: None,
                    reload: vec![],
                },
            )
            .unwrap();
        let completion = worker
            .rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        assert_eq!(completion.id, 1);
        assert!(completion.target == Target::Load);
        let error = completion.result.err().unwrap();
        assert_eq!(
            error.downcast_ref::<std::io::Error>().unwrap().kind(),
            std::io::ErrorKind::NotFound
        );
        assert!(format!("{error:#}").starts_with(&format!("Read {}: ", path.display())));
        assert_eq!(error.chain().count(), 2);
    }
}
