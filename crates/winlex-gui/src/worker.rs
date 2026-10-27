//! A single background worker. Results own their data and never contain HWNDs.
use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender, TrySendError},
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
    requests: Arc<Requests>,
}
impl Drop for Results {
    fn drop(&mut self) {
        self.requests.close();
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
impl Target {
    fn replaces(self, other: Self) -> bool {
        match (self, other) {
            (Self::Document(a) | Self::Reopen(a), Self::Document(b) | Self::Reopen(b)) => a == b,
            _ => self == other,
        }
    }
}
impl Job {
    fn target(&self) -> Target {
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
#[derive(Default)]
struct Pending {
    jobs: VecDeque<(Ticket, Job)>,
    active: Option<(Target, Ticket)>,
}
#[derive(Default)]
struct Requests {
    pending: Mutex<Pending>,
    wake: Condvar,
    stop: AtomicBool,
}
impl Requests {
    fn submit(&self, ticket: Ticket, job: Job) -> Result<(), String> {
        let mut pending = self.pending.lock().unwrap();
        if self.stop.load(Ordering::Relaxed) {
            return Err("Catalog worker stopped".into());
        }
        let target = job.target();
        if let Some((key, old)) = &pending.active
            && target.replaces(*key)
        {
            old.cancel();
        }
        pending.jobs.retain(|(old, queued)| {
            if target.replaces(queued.target()) {
                old.cancel();
            }
            !old.cancelled()
        });
        pending.jobs.push_back((ticket, job));
        self.wake.notify_one();
        Ok(())
    }
    fn next(&self) -> Option<(Ticket, Job)> {
        let mut pending = self.pending.lock().unwrap();
        pending.active = None;
        loop {
            if self.stop.load(Ordering::Relaxed) {
                return None;
            }
            if let Some((ticket, job)) = pending.jobs.pop_front() {
                if ticket.cancelled() {
                    continue;
                }
                pending.active = Some((job.target(), ticket.clone()));
                return Some((ticket, job));
            }
            pending = self.wake.wait(pending).unwrap();
        }
    }
    fn cancel(&self, ticket: &Ticket) {
        ticket.cancel();
        self.pending
            .lock()
            .unwrap()
            .jobs
            .retain(|(t, _)| !t.cancelled());
    }
    fn close(&self) {
        self.stop.store(true, Ordering::Relaxed);
        let mut pending = self.pending.lock().unwrap();
        if let Some((_, ticket)) = &pending.active {
            ticket.cancel();
        }
        for (ticket, _) in pending.jobs.drain(..) {
            ticket.cancel();
        }
        self.wake.notify_one();
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
pub struct Worker {
    requests: Arc<Requests>,
    pub rx: Receiver<Completion>,
    ready: Arc<ReadyEvent>,
}
impl Worker {
    pub fn new() -> Result<Self, String> {
        let requests = Arc::new(Requests::default());
        let (results, rx) = mpsc::sync_channel(8);
        let handle = unsafe { CreateEventW(std::ptr::null(), 0, 0, std::ptr::null()) };
        if handle.is_null() {
            return Err(crate::native::error("Create catalog completion event"));
        }
        let ready = Arc::new(ReadyEvent(handle as usize));
        let results = Results {
            tx: Some(results),
            ready: ready.clone(),
            requests: requests.clone(),
        };
        let queue = requests.clone();
        thread::Builder::new()
            .name("winlex-catalog".into())
            .spawn(move || {
                while let Some((ticket, job)) = queue.next() {
                    let target = job.target();
                    let publish = |result| publish(&results, &ticket, target, &queue.stop, result);
                    let result = execute(job, &ticket, &publish);
                    if !ticket.cancelled() && !queue.stop.load(Ordering::Relaxed) {
                        let failed = publish(result).is_err();
                        if failed && !ticket.cancelled() {
                            break;
                        }
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Self {
            requests,
            rx,
            ready,
        })
    }
    pub fn submit(&self, ticket: Ticket, job: Job) -> Result<(), String> {
        self.requests.submit(ticket, job)
    }
    pub fn cancel(&self, ticket: &Ticket) {
        self.requests.cancel(ticket);
    }
    pub fn resignal(&self) {
        self.ready.signal();
    }
    pub fn event(&self) -> HANDLE {
        self.ready.0 as HANDLE
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.requests.close();
        // A running file read may finish, but shutdown never waits on disk I/O.
    }
}
fn publish(
    results: &Results,
    ticket: &Ticket,
    target: Target,
    stopped: &AtomicBool,
    result: Result<Output, String>,
) -> Result<(), String> {
    let mut value = Completion {
        id: ticket.id,
        target,
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
            ..
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
                match matches.len() {
                    0 => Ok(None),
                    1 => Ok(matches.pop()),
                    _ => Err("Current entity is ambiguous".into()),
                }
            })();
            Ok(Output::Reopened {
                catalog: current,
                entity: result?,
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
        Job::Inspect { path, .. } => Catalog::open(path)
            .map(|c| Output::Inspected(Arc::new(c)))
            .map_err(|e| format!("{e:#}")),
        Job::Describe {
            catalog,
            entity,
            arch,
            ..
        } => catalog
            .describe(&entity, Some(arch))
            .map(|value| Output::Description(format!("{value:#}").replace('\n', "\r\n")))
            .map_err(|e| format!("{e:#}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::{Foundation::WAIT_OBJECT_0, System::Threading::WaitForSingleObject};

    #[test]
    fn replacing_and_closing_documents_releases_queued_snapshots() {
        let requests = Requests::default();
        let catalog = Arc::new(
            Catalog::open(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../inputs/Windows.Win32.winmd"
            ))
            .unwrap(),
        );
        let weak = Arc::downgrade(&catalog);
        let old = Ticket::new(1);
        requests
            .submit(
                old.clone(),
                Job::Describe {
                    document: 10,
                    catalog: catalog.clone(),
                    entity: String::new(),
                    arch: Architecture::X64,
                },
            )
            .unwrap();
        let closed = Ticket::new(2);
        requests
            .submit(
                closed.clone(),
                Job::Describe {
                    document: 20,
                    catalog,
                    entity: String::new(),
                    arch: Architecture::X64,
                },
            )
            .unwrap();
        requests
            .submit(
                Ticket::new(3),
                Job::Inspect {
                    document: 30,
                    path: PathBuf::new(),
                },
            )
            .unwrap();
        requests
            .submit(
                Ticket::new(4),
                Job::Inspect {
                    document: 10,
                    path: PathBuf::new(),
                },
            )
            .unwrap();
        assert!(old.cancelled());
        requests.cancel(&closed);
        assert!(
            weak.upgrade().is_none(),
            "queued snapshots must be released before dequeue"
        );
        let (active, _) = requests.next().unwrap();
        assert_eq!(active.id, 3);
        requests
            .submit(
                Ticket::new(5),
                Job::Inspect {
                    document: 30,
                    path: PathBuf::new(),
                },
            )
            .unwrap();
        assert!(active.cancelled());
        assert_eq!(requests.next().unwrap().0.id, 4);
        assert_eq!(requests.next().unwrap().0.id, 5);
        requests.close();
        assert!(requests.next().is_none());
    }
    #[test]
    fn load_burst_keeps_latest_request_without_reordering_other_work() {
        let requests = Requests::default();
        for id in 1..100 {
            requests
                .submit(
                    Ticket::new(id),
                    Job::Load {
                        paths: vec![],
                        previous: vec![],
                        reload: vec![],
                    },
                )
                .unwrap();
            if id == 50 {
                requests
                    .submit(
                        Ticket::new(100),
                        Job::Discover {
                            files: vec![],
                            directories: vec![],
                        },
                    )
                    .unwrap();
            }
        }
        assert_eq!(requests.pending.lock().unwrap().jobs.len(), 2);
        assert_eq!(requests.next().unwrap().0.id, 100);
        assert_eq!(requests.next().unwrap().0.id, 99);
        requests.close();
    }

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
        let worker = Worker::new().unwrap();
        worker.requests.close();
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
                    Job::Inspect {
                        document: id,
                        path: PathBuf::new(),
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
