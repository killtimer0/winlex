//! A single background worker. Results own their data and never contain HWNDs.
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender, SyncSender, TrySendError},
    },
    thread,
};
use windows_sys::Win32::{
    Foundation::{CloseHandle, HANDLE},
    System::Threading::{CreateEventW, SetEvent},
};
use winlex_core::{Architecture, Catalog, EntitySummary, SourceData};

// An event owns no HWND and remains valid until both UI and worker release it.
struct ReadyEvent(usize);
impl ReadyEvent {
    fn signal(&self) {
        unsafe {
            SetEvent(self.0 as HANDLE);
        }
    }
}
impl Drop for ReadyEvent {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0 as HANDLE);
        }
    }
}
struct Results {
    tx: Option<SyncSender<Completion>>,
    ready: Arc<ReadyEvent>,
}
impl Drop for Results {
    fn drop(&mut self) {
        self.tx.take();
        self.ready.signal();
    }
}

#[derive(Clone)]
pub struct Ticket {
    pub id: u64,
    pub cancel: Arc<AtomicBool>,
}
impl Ticket {
    pub fn new(id: u64) -> Self {
        Self {
            id,
            cancel: Arc::new(AtomicBool::new(false)),
        }
    }
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}
pub enum Job {
    Reopen {
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
    Inspected {
        document: u64,
        catalog: Arc<Catalog>,
    },
    Description {
        document: u64,
        text: String,
    },
}
pub struct Completion {
    pub id: u64,
    pub result: Result<Output, String>,
}
pub struct Worker {
    tx: Option<Sender<(Ticket, Job)>>,
    pub rx: Receiver<Completion>,
    stop: Arc<AtomicBool>,
    ready: Arc<ReadyEvent>,
}
impl Worker {
    pub fn new() -> Result<Self, String> {
        let (tx, requests) = mpsc::channel::<(Ticket, Job)>();
        let (results, rx) = mpsc::sync_channel(8);
        let handle = unsafe { CreateEventW(std::ptr::null(), 0, 0, std::ptr::null()) };
        if handle.is_null() {
            return Err(crate::native::error("Create catalog completion event"));
        }
        let ready = Arc::new(ReadyEvent(handle as usize));
        let results = Results {
            tx: Some(results),
            ready: ready.clone(),
        };
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        thread::Builder::new()
            .name("winlex-catalog".into())
            .spawn(move || {
                while let Ok((ticket, job)) = requests.recv() {
                    if stopped.load(Ordering::Relaxed) {
                        break;
                    }
                    if ticket.cancelled() {
                        continue;
                    }
                    let publish = |result| publish(&results, &ticket, &stopped, result);
                    let result = execute(job, &ticket, &publish);
                    if !ticket.cancelled() && !stopped.load(Ordering::Relaxed) {
                        let failed = publish(result).is_err();
                        if failed && !ticket.cancelled() {
                            break;
                        }
                    }
                    results.ready.signal();
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Self {
            tx: Some(tx),
            rx,
            stop,
            ready,
        })
    }
    pub fn submit(&self, ticket: Ticket, job: Job) -> Result<(), String> {
        self.tx
            .as_ref()
            .ok_or_else(|| "Catalog worker stopped".to_owned())?
            .send((ticket, job))
            .map_err(|_| "Catalog worker stopped".into())
    }
    pub fn event(&self) -> HANDLE {
        self.ready.0 as HANDLE
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.tx.take();
        // A running file read may finish, but shutdown never waits on disk I/O.
    }
}
fn publish(
    results: &Results,
    ticket: &Ticket,
    stopped: &AtomicBool,
    result: Result<Output, String>,
) -> Result<(), String> {
    let mut value = Completion {
        id: ticket.id,
        result,
    };
    loop {
        if ticket.cancelled() || stopped.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        match results.tx.as_ref().unwrap().try_send(value) {
            Ok(()) => {
                results.ready.signal();
                return Ok(());
            }
            Err(TrySendError::Disconnected(_)) => return Err("UI closed".into()),
            Err(TrySendError::Full(v)) => {
                value = v;
                thread::sleep(std::time::Duration::from_millis(5));
            }
        }
    }
}
fn execute(
    job: Job,
    ticket: &Ticket,
    publish: &impl Fn(Result<Output, String>) -> Result<(), String>,
) -> Result<Output, String> {
    match job {
        Job::Reopen {
            previous,
            current,
            source,
            entity,
            arch,
        } => {
            let result = (|| -> Result<Option<EntitySummary>, String> {
                let old = previous
                    .describe(&entity, Some(arch))
                    .map_err(|e| e.to_string())?;
                let name = old["entity"]["full_name"]
                    .as_str()
                    .ok_or("Missing full name")?;
                let target = current
                    .sources()
                    .iter()
                    .find(|s| s.id == source)
                    .ok_or("Current source unavailable")?;
                let mut matches = Vec::new();
                current
                    .search_batches(name, 128, Some(&ticket.cancel), |batch| {
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
                    })
                    .map_err(|e| e.to_string())?;
                Ok(if matches.len() == 1 {
                    matches.pop()
                } else {
                    None
                })
            })();
            Ok(Output::Reopened {
                catalog: current,
                entity: result.ok().flatten(),
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
                    return Err("cancelled".into());
                }
                let data = if !reload.contains(&path)
                    && let Some(data) = previous.iter().find(|s| s.path() == path)
                {
                    data.clone()
                } else {
                    SourceData::read(&path).map_err(|e| format!("{}: {e:#}", path.display()))?
                };
                let data = inputs
                    .iter()
                    .find(|s| s.id() == data.id())
                    .map(|s| s.with_path(path))
                    .unwrap_or(data);
                inputs.push(data);
            }
            let catalog =
                Arc::new(Catalog::from_inputs(inputs.clone()).map_err(|e| format!("{e:#}"))?);
            Ok(Output::Loaded(Snapshot {
                catalog: Some(catalog),
                inputs,
            }))
        }
        Job::Search { catalog, query } => catalog
            .search_batches(&query, 128, Some(&ticket.cancel), |batch| {
                publish(Ok(Output::Matches(batch))).map_err(winlex_core::Error::msg)
            })
            .map(Output::SearchDone)
            .map_err(|e| format!("{e:#}")),
        Job::Discover { files, directories } => {
            let mut paths = files;
            let mut entries = Vec::new();
            for directory in directories {
                if ticket.cancelled() {
                    return Err("cancelled".into());
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
                    return Err("cancelled".into());
                }
                // Discovery fingerprints bytes without constructing a Catalog or activating them.
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
        Job::Inspect { document, path } => Catalog::open(path)
            .map(|c| Output::Inspected {
                document,
                catalog: Arc::new(c),
            })
            .map_err(|e| format!("{e:#}")),
        Job::Describe {
            document,
            catalog,
            entity,
            arch,
        } => catalog
            .describe(&entity, Some(arch))
            .map(|value| Output::Description {
                document,
                text: format!("{value:#}").replace('\n', "\r\n"),
            })
            .map_err(|e| format!("{e:#}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::{Foundation::WAIT_OBJECT_0, System::Threading::WaitForSingleObject};

    #[test]
    fn completion_event_wakes_without_ui_polling() {
        let worker = Worker::new().unwrap();
        worker
            .submit(
                Ticket::new(7),
                Job::Load {
                    paths: vec![],
                    previous: vec![],
                    reload: vec![],
                },
            )
            .unwrap();
        assert_eq!(
            unsafe { WaitForSingleObject(worker.event(), 5000) },
            WAIT_OBJECT_0
        );
        let completion = worker.rx.try_recv().unwrap();
        assert_eq!(completion.id, 7);
        assert!(matches!(completion.result, Ok(Output::Loaded(_))));
    }

    #[test]
    fn worker_exit_signals_disconnection_and_rejects_new_jobs() {
        let mut worker = Worker::new().unwrap();
        worker.tx.take();
        assert_eq!(
            unsafe { WaitForSingleObject(worker.event(), 5000) },
            WAIT_OBJECT_0
        );
        assert!(matches!(
            worker.rx.try_recv(),
            Err(mpsc::TryRecvError::Disconnected)
        ));
        assert!(
            worker
                .submit(
                    Ticket::new(8),
                    Job::Load {
                        paths: vec![],
                        previous: vec![],
                        reload: vec![]
                    }
                )
                .is_err()
        );
    }
    #[test]
    fn cancelling_a_full_result_queue_keeps_worker_alive() {
        let worker = Worker::new().unwrap();
        let mut tickets = Vec::new();
        for id in 0..10 {
            let ticket = Ticket::new(id);
            tickets.push(ticket.clone());
            worker
                .submit(
                    ticket,
                    Job::Load {
                        paths: vec![],
                        previous: vec![],
                        reload: vec![],
                    },
                )
                .unwrap();
        }
        // Wait until bounded output fills; the ninth publish must wait or observe cancellation.
        std::thread::sleep(std::time::Duration::from_millis(100));
        for t in tickets {
            t.cancel();
        }
        worker
            .submit(
                Ticket::new(100),
                Job::Load {
                    paths: vec![],
                    previous: vec![],
                    reload: vec![],
                },
            )
            .unwrap();
        loop {
            let c = worker
                .rx
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap();
            if c.id == 100 {
                assert!(c.result.is_ok());
                break;
            }
        }
    }
}
