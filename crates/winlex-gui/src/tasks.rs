//! Metadata operations executed by the application's background executor.
use crate::{
    results::{Builder, Items, ResultItem},
    worker::Ticket,
};
use anyhow::{Context, bail};
use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
use winlex_core::{
    Architecture, Catalog, EntityId, SearchMatch, SearchOptions, SearchStep, SourceData,
};

const SEARCH_BATCH_INTERVAL: Duration = Duration::from_millis(100);

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
    Matches(Items),
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
        Job::Load { inputs } => {
            if inputs.is_empty() {
                return Ok(Output::Loaded(None));
            }
            let mut loaded = Vec::new();
            for input in inputs.iter() {
                if ticket.cancelled() {
                    bail!("cancelled");
                }
                let data = match input {
                    LoadInput::Reuse(data) => data.clone(),
                    LoadInput::Read(path) => SourceData::read(path)
                        .with_context(|| format!("Read {}", path.display()))?,
                };
                loaded.push(data);
            }
            Ok(Output::Loaded(Some(Arc::new(Catalog::from_inputs(
                loaded,
            )?))))
        }
        Job::Search {
            catalog,
            compiled,
            categories,
        } => {
            let mut search = catalog.search_compiled(
                *compiled,
                SearchOptions {
                    cancel: Some(&ticket.cancel),
                    categories: Some(categories),
                    ..Default::default()
                },
            )?;
            let mut builder = Builder::new();
            search_batches(
                ticket,
                || search.step(),
                publish,
                Instant::now,
                |item| builder.prepare(&catalog, item),
            )
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

fn search_batches(
    ticket: &Ticket,
    mut next: impl FnMut() -> anyhow::Result<SearchStep>,
    publish: &impl Fn(anyhow::Result<Output>) -> anyhow::Result<()>,
    mut now: impl FnMut() -> Instant,
    mut prepare: impl FnMut(SearchMatch) -> anyhow::Result<Box<ResultItem>>,
) -> anyhow::Result<Output> {
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
            Ok(SearchStep::Done) => break Ok(Output::SearchDone),
            Err(error) => break Err(error),
        }
        if !batch.is_empty() && now().duration_since(sent) >= SEARCH_BATCH_INTERVAL {
            publish(Ok(Output::Matches(std::mem::take(&mut batch))))?;
            sent = now();
        }
    };
    if !batch.is_empty() {
        publish(Ok(Output::Matches(batch)))?;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        cell::{Cell, RefCell},
        collections::VecDeque,
    };

    fn hit(token: u32) -> SearchStep {
        SearchStep::Match(SearchMatch {
            evidence: Vec::new(),
            entity: winlex_core::EntitySummary {
                id: EntityId {
                    source_id: 0,
                    token,
                },
                name: "File".into(),
                full_name: "Fixture.File".into(),
                kind: "type".into(),
                architecture_mask: 7,
            },
            quality: winlex_core::MatchQuality::Fuzzy {
                score: 1,
                field: winlex_core::MatchField::Name,
            },
        })
    }

    type BatchLog = Vec<(u64, Vec<u32>)>;
    fn prepare(matched: SearchMatch) -> anyhow::Result<Box<ResultItem>> {
        Ok(Box::new(ResultItem {
            matched,
            display: Default::default(),
        }))
    }
    fn collect_batches(
        events: Vec<(u64, anyhow::Result<SearchStep>)>,
        cancel_at: Option<usize>,
    ) -> (anyhow::Result<Output>, BatchLog) {
        let ticket = Ticket::new(1);
        let base = Instant::now();
        let time = Cell::new(base);
        let mut events: VecDeque<_> = events.into();
        let batches = RefCell::new(vec![]);
        let mut index = 0;
        let result = search_batches(
            &ticket,
            || {
                if cancel_at == Some(index) {
                    ticket.cancel();
                }
                index += 1;
                let (millis, step) = events.pop_front().expect("scan after completion");
                time.set(base + Duration::from_millis(millis));
                step
            },
            &|output| {
                let Ok(Output::Matches(batch)) = output else {
                    panic!("unexpected output");
                };
                assert!(!batch.is_empty());
                batches.borrow_mut().push((
                    time.get().duration_since(base).as_millis() as u64,
                    batch.iter().map(|item| item.id.token).collect(),
                ));
                Ok(())
            },
            || time.get(),
            prepare,
        );
        (result, batches.into_inner())
    }

    #[test]
    fn search_batches_ignore_count_and_flush_short_queries_at_completion() {
        let mut events: Vec<_> = (0..1000).map(|id| (id as u64 / 10, Ok(hit(id)))).collect();
        events.push((100, Ok(SearchStep::Done)));
        let (result, batches) = collect_batches(events, None);
        assert!(matches!(result, Ok(Output::SearchDone)));
        assert_eq!(batches, [(100, (0..1000).collect())]);
    }

    #[test]
    fn search_batches_check_elapsed_time_even_when_candidates_do_not_match() {
        let (result, batches) = collect_batches(
            vec![
                (0, Ok(hit(1))),
                (99, Ok(SearchStep::Skipped)),
                (100, Ok(SearchStep::Skipped)),
                (101, Ok(hit(2))),
                (199, Ok(SearchStep::Skipped)),
                (200, Ok(SearchStep::Skipped)),
                (201, Ok(hit(3))),
                (202, Ok(SearchStep::Done)),
            ],
            None,
        );
        assert!(matches!(result, Ok(Output::SearchDone)));
        assert_eq!(batches, [(100, vec![1]), (200, vec![2]), (202, vec![3])]);
    }

    #[test]
    fn search_batches_do_not_emit_empty_batches_and_flush_before_error() {
        let (result, batches) = collect_batches(
            vec![
                (100, Ok(SearchStep::Skipped)),
                (200, Ok(SearchStep::Skipped)),
                (240, Ok(hit(1))),
                (250, Ok(hit(2))),
                (260, Err(anyhow::anyhow!("scan failed"))),
            ],
            None,
        );
        assert_eq!(result.err().unwrap().to_string(), "scan failed");
        assert_eq!(batches, [(240, vec![1]), (260, vec![2])]);
        let (result, batches) = collect_batches(vec![(1000, Ok(SearchStep::Done))], None);
        assert!(matches!(result, Ok(Output::SearchDone)));
        assert!(batches.is_empty());
    }

    #[test]
    fn search_batches_discard_unsent_results_on_cancellation() {
        let (result, batches) = collect_batches(
            vec![
                (0, Ok(hit(1))),
                (100, Ok(SearchStep::Skipped)),
                (120, Ok(hit(2))),
                (200, Ok(SearchStep::Done)),
            ],
            Some(3),
        );
        assert_eq!(result.err().unwrap().to_string(), "cancelled");
        assert_eq!(batches, [(100, vec![1])]);
    }

    #[test]
    fn search_batches_propagate_publish_failure_without_scanning_further() {
        let mut count = 0;
        let base = Instant::now();
        let time = Cell::new(base);
        let result = search_batches(
            &Ticket::new(1),
            || {
                count += 1;
                time.set(base + SEARCH_BATCH_INTERVAL);
                Ok(hit(1))
            },
            &|_| Err(anyhow::anyhow!("queue closed")),
            || time.get(),
            prepare,
        );
        assert_eq!(result.err().unwrap().to_string(), "queue closed");
        assert_eq!(count, 1);
    }

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
                    inputs: Arc::from([LoadInput::Read(path.clone())]),
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
    }
}
