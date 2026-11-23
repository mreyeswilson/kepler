//! The single write coordinator: one thread owns the only writable connection.

use crate::memory::{store_memory, NewMemory};
use crate::records::{EventRecord, MemoryRecord};
use crate::{open_rw, StorageError};
use rusqlite::{Connection, Transaction};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TrySendError};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct WriterConfig {
    /// Bounded queue length; when full, events are dropped (counted), never blocked on.
    pub queue_capacity: usize,
    /// Max commands applied in one short transaction.
    pub batch_size: usize,
    /// Keep at most this many event rows (oldest pruned).
    pub max_events: i64,
}

impl Default for WriterConfig {
    fn default() -> Self {
        Self {
            queue_capacity: 256,
            batch_size: 64,
            max_events: 10_000,
        }
    }
}

/// A unit of work run on the writer thread inside its own savepoint: `true` commits the
/// savepoint, `false` rolls it back. The returned `Finish` runs after the batch transaction
/// committed (`true`) or failed (`false`), so callers never see a result before it is durable.
type Finish = Box<dyn FnOnce(bool) + Send>;
type Job = Box<dyn FnOnce(&Connection) -> (bool, Finish) + Send>;

enum Cmd {
    Event(Box<EventRecord>),
    Job(Job),
    Flush(SyncSender<()>),
}

pub struct Writer {
    tx: Option<SyncSender<Cmd>>,
    dropped: Arc<AtomicU64>,
    failed: Arc<AtomicU64>,
    join: Option<JoinHandle<()>>,
}

impl Writer {
    /// Open (and migrate) the database on the caller's thread so failures are
    /// reported immediately, then hand the connection to the writer thread.
    pub fn start(path: &Path, cfg: WriterConfig) -> Result<Self, StorageError> {
        if let Some(dir) = path.parent() {
            crate::dirs::ensure_private_dir(dir)?;
        }
        let conn = open_rw(path)?;
        let (tx, rx) = sync_channel::<Cmd>(cfg.queue_capacity.max(1));
        let failed = Arc::new(AtomicU64::new(0));
        let failed2 = failed.clone();
        let join = std::thread::Builder::new()
            .name("kepler-storage-writer".into())
            .spawn(move || run(conn, rx, cfg, failed2))
            .map_err(StorageError::Io)?;
        Ok(Self {
            tx: Some(tx),
            dropped: Arc::new(AtomicU64::new(0)),
            failed,
            join: Some(join),
        })
    }

    /// Queue an event record without blocking. Returns `false` if it was dropped
    /// (queue full or writer gone): persistence is an optimization, never a gate.
    pub fn record_event(&self, rec: EventRecord) -> bool {
        let Some(tx) = &self.tx else { return false };
        match tx.try_send(Cmd::Event(Box::new(rec))) {
            Ok(()) => true,
            Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) => {
                self.dropped.fetch_add(1, Ordering::Relaxed);
                false
            }
        }
    }

    /// Store a memory (redacted, path-checked) and return its row id.
    /// Blocks the calling thread up to `timeout`; call from a blocking context.
    pub fn add_memory(&self, rec: MemoryRecord, timeout: Duration) -> Result<i64, StorageError> {
        let mut m = NewMemory::new(&rec.project_id, &rec.content, &rec.source);
        m.source_path = rec.source_path.clone();
        m.source_kind = "api".into();
        self.store(m, timeout).map(|o| o.id())
    }

    /// Sanitize (redact, path-check) and store a memory with dedupe/supersession.
    /// Blocks the calling thread up to `timeout`; call from a blocking context.
    pub fn store(
        &self,
        m: NewMemory,
        timeout: Duration,
    ) -> Result<crate::memory::StoreOutcome, StorageError> {
        let content = m.sanitize()?;
        self.exec(timeout, move |conn| store_memory(conn, &m, &content))
    }

    /// Run `f` on the writer thread (atomically, in its own savepoint) and wait for its result.
    pub fn exec<T, F>(&self, timeout: Duration, f: F) -> Result<T, StorageError>
    where
        T: Send + 'static,
        F: FnOnce(&Connection) -> Result<T, StorageError> + Send + 'static,
    {
        let (reply, wait) = sync_channel(1);
        let tx = self.tx.as_ref().ok_or(StorageError::WriterUnavailable)?;
        let job: Job = Box::new(move |conn| {
            let res = f(conn);
            let ok = res.is_ok();
            let finish: Finish = Box::new(move |committed| {
                let _ = reply.send(if committed {
                    res
                } else {
                    Err(StorageError::WriterUnavailable)
                });
            });
            (ok, finish)
        });
        tx.try_send(Cmd::Job(job))
            .map_err(|_| StorageError::WriterUnavailable)?;
        wait.recv_timeout(timeout)
            .map_err(|_| StorageError::WriterUnavailable)?
    }

    /// Fire-and-forget variant of [`Writer::exec`]: never blocks, drops (and counts) when the
    /// queue is full. For optional bookkeeping such as metrics and usage counters.
    pub fn submit<F>(&self, f: F) -> bool
    where
        F: FnOnce(&Connection) -> Result<(), StorageError> + Send + 'static,
    {
        let Some(tx) = &self.tx else { return false };
        let job: Job = Box::new(move |conn| (f(conn).is_ok(), Box::new(|_| {})));
        match tx.try_send(Cmd::Job(job)) {
            Ok(()) => true,
            Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) => {
                self.dropped.fetch_add(1, Ordering::Relaxed);
                false
            }
        }
    }

    /// Wait until everything queued before this call is committed.
    pub fn flush(&self, timeout: Duration) -> bool {
        let Some(tx) = &self.tx else { return false };
        let (reply, wait) = sync_channel(1);
        if tx.send_timeout_compat(Cmd::Flush(reply), timeout).is_err() {
            return false;
        }
        wait.recv_timeout(timeout).is_ok()
    }

    pub fn dropped_events(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    pub fn failed_writes(&self) -> u64 {
        self.failed.load(Ordering::Relaxed)
    }
}

/// `SyncSender` has no `send_timeout`; poll `try_send` for a short, bounded time.
trait SendTimeoutCompat<T> {
    fn send_timeout_compat(&self, v: T, timeout: Duration) -> Result<(), ()>;
}

impl<T> SendTimeoutCompat<T> for SyncSender<T> {
    fn send_timeout_compat(&self, mut v: T, timeout: Duration) -> Result<(), ()> {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            match self.try_send(v) {
                Ok(()) => return Ok(()),
                Err(TrySendError::Disconnected(_)) => return Err(()),
                Err(TrySendError::Full(back)) => {
                    if std::time::Instant::now() >= deadline {
                        return Err(());
                    }
                    v = back;
                    std::thread::sleep(Duration::from_millis(2));
                }
            }
        }
    }
}

impl Drop for Writer {
    fn drop(&mut self) {
        self.tx = None; // disconnect: the thread drains what is queued, then exits
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

fn run(mut conn: Connection, rx: Receiver<Cmd>, cfg: WriterConfig, failed: Arc<AtomicU64>) {
    let mut since_prune = 0u64;
    loop {
        // Blocking wait (no polling); wake-up cost only when there is work.
        let first = match rx.recv() {
            Ok(c) => c,
            Err(_) => return,
        };
        let mut batch = vec![first];
        while batch.len() < cfg.batch_size.max(1) {
            match rx.try_recv() {
                Ok(c) => batch.push(c),
                Err(_) => break,
            }
        }
        let mut flushes = Vec::new();
        let mut finishers: Vec<Finish> = Vec::new();
        let mut events_written = 0u64;
        match conn.transaction() {
            Err(_) => {
                failed.fetch_add(batch.len() as u64, Ordering::Relaxed);
                for c in batch {
                    match c {
                        Cmd::Job(job) => {
                            // Not run: tell the caller the writer could not take it.
                            drop(job);
                        }
                        Cmd::Flush(r) => {
                            let _ = r.send(());
                        }
                        Cmd::Event(_) => {}
                    }
                }
                continue;
            }
            Ok(mut tx) => {
                for c in batch {
                    match c {
                        Cmd::Event(rec) => match insert_event(&tx, &rec) {
                            Ok(n) => events_written += n,
                            Err(_) => {
                                failed.fetch_add(1, Ordering::Relaxed);
                            }
                        },
                        Cmd::Job(job) => match tx.savepoint() {
                            Ok(mut sp) => {
                                let (ok, finish) = job(&sp);
                                if ok {
                                    let _ = sp.commit();
                                } else {
                                    let _ = sp.rollback();
                                    failed.fetch_add(1, Ordering::Relaxed);
                                }
                                finishers.push(finish);
                            }
                            Err(_) => {
                                failed.fetch_add(1, Ordering::Relaxed);
                            }
                        },
                        Cmd::Flush(r) => flushes.push(r),
                    }
                }
                let committed = tx.commit().is_ok();
                if !committed {
                    failed.fetch_add(1, Ordering::Relaxed);
                }
                for f in finishers.drain(..) {
                    f(committed);
                }
            }
        }
        for f in flushes {
            let _ = f.send(());
        }
        since_prune += events_written;
        if since_prune >= 500 {
            since_prune = 0;
            let _ = conn.execute(
                "DELETE FROM events WHERE id <= (SELECT MAX(id) FROM events) - ?1",
                [cfg.max_events],
            );
        }
    }
}

/// Idempotent on (session_id, event_id): retries never duplicate rows.
fn insert_event(tx: &Transaction<'_>, r: &EventRecord) -> rusqlite::Result<u64> {
    let inserted = tx.execute(
        "INSERT OR IGNORE INTO events
           (event_id, kind, ts, harness, project_id, session_id, correlation_id, payload_bytes, decision, reason)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        rusqlite::params![
            r.event_id,
            r.kind,
            r.ts,
            r.harness,
            r.project_id,
            r.session_id,
            r.correlation_id,
            r.payload_bytes,
            r.decision,
            r.reason
        ],
    )?;
    if inserted == 1 {
        let closed = i64::from(r.kind == "session.closed");
        tx.execute(
            "INSERT INTO sessions (session_id, harness, project_id, first_seen, last_seen, event_count, closed)
             VALUES (?1, ?2, ?3, ?4, ?4, 1, ?5)
             ON CONFLICT(session_id) DO UPDATE SET
               last_seen = excluded.last_seen,
               event_count = event_count + 1,
               closed = MAX(closed, excluded.closed)",
            rusqlite::params![r.session_id, r.harness, r.project_id, r.ts, closed],
        )?;
    }
    Ok(inserted as u64)
}
