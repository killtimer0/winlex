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
    Foundation::HANDLE,
    System::Threading::{CreateEventW, SetEvent},
};

struct ReadyEvent(OwnedHandle);
impl ReadyEvent {
    fn signal(&self) {
        unsafe {
            SetEvent(self.0.as_raw_handle() as HANDLE);
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
impl<J, O> Drop for Results<J, O> {
    fn drop(&mut self) {
        self.requests.close();
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
        anyhow::ensure!(threads > 0, "Worker requires at least one thread");
        let requests = Arc::new(Requests::default());
        let (tx, rx) = mpsc::sync_channel(8);
        let handle = unsafe { CreateEventW(std::ptr::null(), 0, 0, std::ptr::null()) };
        if handle.is_null() {
            return Err(crate::native::error("Create completion event"));
        }
        let ready = Arc::new(ReadyEvent(unsafe { OwnedHandle::from_raw_handle(handle) }));
        let results = Arc::new(Results {
            tx: Some(tx),
            ready: ready.clone(),
            requests: requests.clone(),
        });
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
            thread::Builder::new()
                .name(name)
                .spawn(move || {
                    let mut previous = None;
                    while let Some((ticket, job)) = queue.next(previous) {
                        let publish = |value| publish(&results, &ticket, value);
                        execute(job, &ticket, &publish);
                        previous = Some(ticket.id);
                    }
                })
                .context("Start background worker")?;
        }
        Ok(Self {
            requests,
            rx,
            ready,
        })
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
    pub fn event(&self) -> HANDLE {
        self.ready.0.as_raw_handle() as HANDLE
    }
}
impl<J, O> Drop for Worker<J, O> {
    fn drop(&mut self) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::{Foundation::WAIT_OBJECT_0, System::Threading::WaitForSingleObject};
    #[test]
    fn replacement_releases_payload_and_preserves_other_order() {
        let requests = Requests::default();
        let payload = Arc::new(());
        let weak = Arc::downgrade(&payload);
        let old = Ticket::new(1);
        requests.replace(None, old.clone(), Some(payload)).unwrap();
        requests.replace(None, Ticket::new(2), None).unwrap();
        requests.replace(Some(1), Ticket::new(3), None).unwrap();
        assert!(old.cancelled());
        assert!(weak.upgrade().is_none());
        let active = requests.next(None).unwrap().0;
        assert_eq!(active.id, 2);
        requests.replace(Some(2), Ticket::new(4), None).unwrap();
        assert!(active.cancelled());
        assert_eq!(requests.next(Some(2)).unwrap().0.id, 3);
        assert_eq!(requests.next(Some(3)).unwrap().0.id, 4);
        requests.close();
        assert!(requests.next(Some(4)).is_none());
    }
    #[test]
    fn cancellation_targets_each_running_job() {
        let requests = Requests::default();
        let first = Ticket::new(1);
        let second = Ticket::new(2);
        requests.replace(None, first.clone(), ()).unwrap();
        requests.replace(None, second.clone(), ()).unwrap();
        let a = requests.next(None).unwrap().0;
        let b = requests.next(None).unwrap().0;
        assert_eq!((a.id, b.id), (1, 2));
        requests.cancel(2);
        assert!(!a.cancelled());
        assert!(b.cancelled());
        requests.cancel(1);
        assert!(a.cancelled());
        requests.close();
    }
    #[test]
    fn completion_and_shutdown_wake_ui() {
        let worker = Worker::new(|(), ticket, publish| {
            let _ = publish(ticket.id);
        })
        .unwrap();
        worker.replace(None, Ticket::new(7), ()).unwrap();
        assert_eq!(
            unsafe { WaitForSingleObject(worker.event(), 5000) },
            WAIT_OBJECT_0
        );
        assert_eq!(worker.rx.try_recv().unwrap(), 7);
        worker.requests.close();
        assert_eq!(
            unsafe { WaitForSingleObject(worker.event(), 5000) },
            WAIT_OBJECT_0
        );
        assert!(matches!(
            worker.rx.try_recv(),
            Err(mpsc::TryRecvError::Disconnected)
        ));
        assert!(worker.replace(None, Ticket::new(8), ()).is_err());
    }
    #[test]
    fn two_threads_run_jobs_concurrently() {
        use std::sync::Barrier;
        let barrier = Arc::new(Barrier::new(2));
        let worker = Worker::with_threads(
            {
                let barrier = barrier.clone();
                move |(), _, publish| {
                    barrier.wait();
                    let _ = publish(());
                }
            },
            2,
        )
        .unwrap();
        worker.replace(None, Ticket::new(1), ()).unwrap();
        worker.replace(None, Ticket::new(2), ()).unwrap();
        for _ in 0..2 {
            worker
                .rx
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap();
        }
    }
    #[test]
    fn cancelling_full_output_keeps_executor_alive() {
        let (started, ready) = mpsc::channel();
        let started = std::sync::Mutex::new(started);
        let worker = Worker::with_threads(
            move |(), ticket, publish| {
                if ticket.id == 9 {
                    started.lock().unwrap().send(()).unwrap();
                }
                let _ = publish(ticket.id);
            },
            1,
        )
        .unwrap();
        let tickets: Vec<_> = (1..=9).map(Ticket::new).collect();
        for ticket in &tickets {
            worker.replace(None, ticket.clone(), ()).unwrap();
        }
        ready
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        worker.cancel(tickets[8].id);
        worker.replace(None, Ticket::new(10), ()).unwrap();
        for _ in 0..8 {
            worker
                .rx
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap();
        }
        assert_eq!(
            worker
                .rx
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap(),
            10
        );
    }
}
