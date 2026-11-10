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
            completion: tools::run(job),
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        cell::{Cell, RefCell},
        collections::VecDeque,
    };
    use winlex_core::EntityId;

    #[test]
    fn tool_completion_keeps_document_ticket_and_input_identity() {
        let ticket = Ticket::new(17);
        run(
            Job::Tool {
                document: 23,
                job: tools::Job::Languages {
                    path: PathBuf::new(),
                },
            },
            &ticket,
            &|completion| {
                let Completion::Tool {
                    id,
                    document,
                    completion: tools::Completion::Languages { path, result },
                } = completion
                else {
                    panic!("expected tool language completion");
                };
                assert_eq!(id, 17);
                assert_eq!(document, 23);
                assert!(path.as_os_str().is_empty());
                assert!(result.is_err());
                Ok(())
            },
        );
    }

    #[test]
    fn cancelled_tool_job_does_not_publish() {
        let ticket = Ticket::new(1);
        ticket.cancel();
        run(
            Job::Tool {
                document: 1,
                job: tools::Job::Languages {
                    path: PathBuf::new(),
                },
            },
            &ticket,
            &|_| panic!("cancelled tool job published a completion"),
        );
    }

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
    fn batch_tokens(batches: BatchLog) -> Vec<u32> {
        batches.into_iter().flat_map(|(_, batch)| batch).collect()
    }

    fn prepare(matched: SearchMatch) -> anyhow::Result<Box<ResultItem>> {
        Ok(Box::new(ResultItem {
            matched,
            display: Default::default(),
        }))
    }
    fn collect_batches(
        events: Vec<(u64, anyhow::Result<SearchStep>)>,
        cancel_at: Option<usize>,
    ) -> (anyhow::Result<()>, BatchLog) {
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
            &|batch| {
                assert!(!batch.is_empty());
                batches.borrow_mut().push((
                    time.get().duration_since(base).as_millis() as u64,
                    batch.iter().map(|item| item.matched.id.token).collect(),
                ));
                Ok(())
            },
            || time.get(),
            prepare,
        );
        (result, batches.into_inner())
    }

    #[test]
    fn search_batches_flush_short_queries_at_completion() {
        let (result, batches) = collect_batches(
            vec![(0, Ok(hit(1))), (0, Ok(hit(2))), (0, Ok(SearchStep::Done))],
            None,
        );
        assert!(result.is_ok());
        assert_eq!(batch_tokens(batches), [1, 2]);
    }

    #[test]
    fn search_batches_check_elapsed_time_even_when_candidates_do_not_match() {
        let (result, batches) = collect_batches(
            vec![
                (0, Ok(hit(1))),
                (1000, Ok(SearchStep::Skipped)),
                (2000, Ok(SearchStep::Done)),
            ],
            None,
        );
        assert!(result.is_ok());
        assert_eq!(batches.len(), 1);
        assert!(
            batches[0].0 < 2000,
            "pending matches must publish before completion"
        );
        assert_eq!(batches[0].1, [1]);
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
        assert_eq!(batch_tokens(batches), [1, 2]);
        let (result, batches) = collect_batches(vec![(1000, Ok(SearchStep::Done))], None);
        assert!(result.is_ok());
        assert!(batches.is_empty());
    }

    #[test]
    fn search_batches_discard_unsent_results_on_cancellation() {
        let (result, batches) = collect_batches(
            vec![
                (0, Ok(hit(1))),
                (1000, Ok(SearchStep::Skipped)),
                (1000, Ok(hit(2))),
                (1000, Ok(SearchStep::Done)),
            ],
            Some(3),
        );
        assert_eq!(result.err().unwrap().to_string(), "cancelled");
        assert_eq!(batch_tokens(batches), [1]);
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
    fn job_completions_preserve_request_and_document_identity() {
        let catalog = crate::test_support::catalog();
        let worker = crate::worker::Worker::new(run).unwrap();
        let submit = |id, job| worker.replace(None, Ticket::new(id), job).unwrap();
        let receive = || worker.rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let receive_description = |expected| {
            let Completion::Description {
                id,
                document,
                result,
            } = receive()
            else {
                panic!("expected description completion");
            };
            assert_eq!((id, document), expected);
            result
        };
        submit(
            1,
            Job::Search {
                catalog: catalog.clone(),
                compiled: Box::new(
                    winlex_core::CompiledQuery::compile(
                        "__winlex_missing_protocol_contract_entity__",
                        winlex_core::SearchMode::Name,
                        false,
                    )
                    .unwrap(),
                ),
                categories: winlex_core::EntityCategory::ALL,
            },
        );
        let Completion::SearchDone { id, result } = receive() else {
            panic!("expected empty search completion");
        };
        assert_eq!(id, 1);
        result.unwrap();
        let entity = crate::test_support::first_match(catalog.search(
            "Windows.Win32.Security.SECURITY_ATTRIBUTES",
            SearchOptions {
                mode: winlex_core::SearchMode::Name,
                categories: Some(winlex_core::EntityCategory::Type as u32),
                ..Default::default()
            },
        ))
        .id;
        submit(
            2,
            Job::Describe {
                document: 42,
                catalog: catalog.clone(),
                target: entity.into(),
                arch: Architecture::X64,
            },
        );
        let result = receive_description((2, 42));
        assert!(
            result
                .unwrap()
                .regions
                .iter()
                .any(|region| matches!(&region.kind,
                    winlex_format::declaration::RegionKind::Number(number)
                    if number.entity == entity && number.value == 24
                        && number.unit == Some("bytes")))
        );

        submit(
            3,
            Job::Describe {
                document: 43,
                catalog,
                target: EntityId {
                    source_id: 0,
                    token: 0,
                }
                .into(),
                arch: Architecture::X64,
            },
        );
        assert!(receive_description((3, 43)).is_err());

        submit(
            4,
            Job::Load {
                inputs: Arc::from([]),
            },
        );
        let Completion::Loaded { id, result } = receive() else {
            panic!("expected empty catalog completion");
        };
        assert_eq!(id, 4);
        assert!(result.unwrap().is_none());
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
        let Completion::Loaded { id, result } = completion else {
            panic!("expected load completion");
        };
        assert_eq!(id, 1);
        let error = result.err().unwrap();
        assert_eq!(
            error.downcast_ref::<std::io::Error>().unwrap().kind(),
            std::io::ErrorKind::NotFound
        );
        assert!(format!("{error:#}").starts_with(&format!("Read {}: ", path.display())));
    }
}
