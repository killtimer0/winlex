//! Bounded worker pool with explicit replacement, cooperative cancellation and bounded output.
use anyhow::Context;
use std::{
    collections::VecDeque,
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender, TrySendError},
    },
    thread,
};
use windows_sys::Win32::{
    Foundation::{HANDLE, HWND},
    System::Threading::{CreateEventW, SetEvent},
    UI::WindowsAndMessaging::PostMessageW,
};

#[derive(Default)]
struct WindowNotification {
    window: usize,
    message: u32,
    pending: bool,
}
struct ReadyEvent {
    handle: OwnedHandle,
    notification: Mutex<WindowNotification>,
}
impl ReadyEvent {
    fn signal(&self) {
        unsafe {
            SetEvent(self.handle.as_raw_handle() as HANDLE);
        }
        let mut target = self.notification.lock().unwrap();
        if target.window != 0 && !target.pending {
            target.pending =
                unsafe { PostMessageW(target.window as HWND, target.message, 0, 0) != 0 };
        }
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
    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}
struct Pending<J> {
    jobs: VecDeque<(Ticket, J)>,
    active: Vec<Ticket>,
}
struct Requests<J> {
    pending: Mutex<Pending<J>>,
    wake: Condvar,
    stop: AtomicBool,
}
impl<J> Default for Requests<J> {
    fn default() -> Self {
        Self {
            pending: Mutex::new(Pending {
                jobs: VecDeque::new(),
                active: Vec::new(),
            }),
            wake: Condvar::new(),
            stop: AtomicBool::new(false),
        }
    }
}
impl<J> Requests<J> {
    fn replace(&self, old: Option<u64>, ticket: Ticket, job: J) -> anyhow::Result<()> {
        let mut pending = self.pending.lock().unwrap();
        if self.stop.load(Ordering::Relaxed) {
            anyhow::bail!("Background worker stopped");
        }
        if let Some(id) = old {
            Self::cancel_locked(&mut pending, id);
        }
        pending.jobs.push_back((ticket, job));
        self.wake.notify_one();
        Ok(())
    }
    fn cancel_locked(pending: &mut Pending<J>, id: u64) {
        for active in &pending.active {
            if active.id == id {
                active.cancel();
            }
        }
        pending.jobs.retain(|(ticket, _)| {
            if ticket.id == id {
                ticket.cancel();
            }
            !ticket.cancelled()
        });
    }
    fn cancel(&self, id: u64) {
        Self::cancel_locked(&mut self.pending.lock().unwrap(), id);
    }
    fn next(&self, previous: Option<u64>) -> Option<(Ticket, J)> {
        let mut pending = self.pending.lock().unwrap();
        if let Some(id) = previous {
            pending.active.retain(|ticket| ticket.id != id);
        }
        loop {
            if self.stop.load(Ordering::Relaxed) {
                return None;
            }
            if let Some((ticket, job)) = pending.jobs.pop_front() {
                if ticket.cancelled() {
                    continue;
                }
                pending.active.push(ticket.clone());
                return Some((ticket, job));
            }
            pending = self.wake.wait(pending).unwrap();
        }
    }
    fn close(&self) {
        self.stop.store(true, Ordering::Relaxed);
        let mut pending = self.pending.lock().unwrap();
        for ticket in &pending.active {
            ticket.cancel();
        }
        for (ticket, _) in pending.jobs.drain(..) {
            ticket.cancel();
        }
        self.wake.notify_all();
    }
}
struct Results<J, O> {
    tx: Option<SyncSender<O>>,
    ready: Arc<ReadyEvent>,
    requests: Arc<Requests<J>>,
}
struct Running<J, O>(Arc<Results<J, O>>);
impl<J, O> Drop for Running<J, O> {
    fn drop(&mut self) {
        self.0.requests.close();
        self.0.ready.signal();
    }
}
impl<J, O> Drop for Results<J, O> {
    fn drop(&mut self) {
        self.tx.take();
        self.ready.signal();
    }
}
pub struct Worker<J, O> {
    requests: Arc<Requests<J>>,
    pub rx: Receiver<O>,
    ready: Arc<ReadyEvent>,
}
/// Worker threads. One restores strict FIFO; two keep interactive jobs from
/// waiting behind a catalog load.
const THREADS: usize = 2;
impl<J: Send + 'static, O: Send + 'static> Worker<J, O> {
    pub fn new(
        execute: impl Fn(J, &Ticket, &dyn Fn(O) -> anyhow::Result<()>) + Send + Sync + 'static,
    ) -> anyhow::Result<Self> {
        Self::with_threads(execute, THREADS)
    }
    fn with_threads(
        execute: impl Fn(J, &Ticket, &dyn Fn(O) -> anyhow::Result<()>) + Send + Sync + 'static,
        threads: usize,
    ) -> anyhow::Result<Self> {
        Self::with_spawner(execute, threads, |builder, run| builder.spawn(run))
    }
    fn with_spawner(
        execute: impl Fn(J, &Ticket, &dyn Fn(O) -> anyhow::Result<()>) + Send + Sync + 'static,
        threads: usize,
        mut spawn: impl FnMut(
            thread::Builder,
            Box<dyn FnOnce() + Send>,
        ) -> std::io::Result<thread::JoinHandle<()>>,
    ) -> anyhow::Result<Self> {
        anyhow::ensure!(threads > 0, "Worker requires at least one thread");
        let requests = Arc::new(Requests::default());
        let (tx, rx) = mpsc::sync_channel(8);
        let handle = unsafe { CreateEventW(std::ptr::null(), 0, 0, std::ptr::null()) };
        if handle.is_null() {
            return Err(crate::native::error("Create completion event"));
        }
        let ready = Arc::new(ReadyEvent {
            handle: unsafe { OwnedHandle::from_raw_handle(handle) },
            notification: Mutex::new(WindowNotification::default()),
        });
        let results = Arc::new(Results {
            tx: Some(tx),
            ready: ready.clone(),
            requests: requests.clone(),
        });
        let worker = Self {
            requests: requests.clone(),
            rx,
            ready,
        };
        let execute = Arc::new(execute);
        for index in 0..threads {
            let queue = requests.clone();
            let results = results.clone();
            let execute = execute.clone();
            let name = if threads == 1 {
                "winlex-worker".to_owned()
            } else {
                format!("winlex-worker-{index}")
            };
            spawn(
                thread::Builder::new().name(name),
                Box::new(move || {
                    let running = Running(results);
                    let mut previous = None;
                    while let Some((ticket, job)) = queue.next(previous) {
                        let publish = |value| publish(&running.0, &ticket, value);
                        execute(job, &ticket, &publish);
                        previous = Some(ticket.id);
                    }
                }),
            )
            .context("Start background worker")?;
        }
        Ok(worker)
    }
    pub fn replace(&self, old: Option<u64>, ticket: Ticket, job: J) -> anyhow::Result<()> {
        self.requests.replace(old, ticket, job)
    }
    pub fn cancel(&self, id: u64) {
        self.requests.cancel(id);
    }
    pub fn resignal(&self) {
        self.ready.signal();
    }
    pub fn notify_window(&self, window: HWND, message: u32) {
        *self.ready.notification.lock().unwrap() = WindowNotification {
            window: window as usize,
            message,
            pending: false,
        };
    }
    pub fn acknowledge_notification(&self) {
        self.ready.notification.lock().unwrap().pending = false;
    }
    pub fn try_completion(&self) -> Result<O, mpsc::TryRecvError> {
        match self.rx.try_recv() {
            Err(mpsc::TryRecvError::Empty) if self.requests.stop.load(Ordering::Relaxed) => {
                Err(mpsc::TryRecvError::Disconnected)
            }
            result => result,
        }
    }
    pub fn event(&self) -> HANDLE {
        self.ready.handle.as_raw_handle() as HANDLE
    }
}
impl<J, O> Drop for Worker<J, O> {
    fn drop(&mut self) {
        self.ready.notification.lock().unwrap().window = 0;
        self.requests.close();
    }
}
fn publish<J, O>(results: &Results<J, O>, ticket: &Ticket, mut value: O) -> anyhow::Result<()> {
    loop {
        if ticket.cancelled() || results.requests.stop.load(Ordering::Relaxed) {
            anyhow::bail!("cancelled");
        }
        match results.tx.as_ref().unwrap().try_send(value) {
            Ok(()) => {
                results.ready.signal();
                return Ok(());
            }
            Err(TrySendError::Disconnected(_)) => {
                results.requests.close();
                anyhow::bail!("UI closed");
            }
            Err(TrySendError::Full(v)) => {
                value = v;
                thread::sleep(std::time::Duration::from_millis(5));
            }
        }
    }
}
