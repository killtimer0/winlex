//! Independent background execution for one error-code tool instance.
use std::{
    cell::{Cell, RefCell},
    path::PathBuf,
    sync::mpsc,
    thread,
};
use winlex_core::tools::error_code::{self as core, QueryResult, Request};

pub(super) enum Completion {
    Query {
        revision: u64,
        result: anyhow::Result<QueryResult>,
    },
    Languages {
        path: PathBuf,
        result: Result<Vec<u16>, core::Failure>,
    },
}

pub(super) struct Executor {
    tx: mpsc::Sender<Completion>,
    rx: RefCell<mpsc::Receiver<Completion>>,
    pending: Cell<usize>,
}

impl Executor {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            tx,
            rx: RefCell::new(rx),
            pending: Cell::new(0),
        }
    }

    pub fn query(&self, request: Request, revision: u64) -> anyhow::Result<()> {
        self.start(move |tx| {
            let result = core::query(&request);
            let _ = tx.send(Completion::Query { revision, result });
        })
    }

    pub fn languages(&self, path: PathBuf) -> anyhow::Result<()> {
        self.start(move |tx| {
            let result = core::module_languages(&path);
            let _ = tx.send(Completion::Languages { path, result });
        })
    }

    pub fn pending(&self) -> usize {
        self.pending.get()
    }

    pub fn poll(&self) -> Option<Completion> {
        let message = self.rx.borrow_mut().try_recv().ok()?;
        self.pending.set(self.pending.get().saturating_sub(1));
        Some(message)
    }

    fn start(
        &self,
        job: impl FnOnce(mpsc::Sender<Completion>) + Send + 'static,
    ) -> anyhow::Result<()> {
        let tx = self.tx.clone();
        thread::Builder::new()
            .name("winlex-error-code".into())
            .spawn(move || job(tx))?;
        self.pending.set(self.pending.get() + 1);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn pending_completions_are_counted_until_polled() {
        let executor = Executor::new();
        executor
            .start(|tx| {
                tx.send(Completion::Languages {
                    path: std::path::PathBuf::from("messages.dll"),
                    result: Ok(vec![0x409]),
                })
                .unwrap();
            })
            .unwrap();
        assert_eq!(executor.pending(), 1);
        let deadline = Instant::now() + Duration::from_secs(5);
        let completion = loop {
            if let Some(completion) = executor.poll() {
                break completion;
            }
            assert!(Instant::now() < deadline, "completion was not published");
            std::thread::yield_now();
        };
        assert!(matches!(completion, Completion::Languages { .. }));
        assert_eq!(executor.pending(), 0);
        assert!(executor.poll().is_none());
    }
}
