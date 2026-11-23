//! The single write coordinator: one thread owns the only writable connection.

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

type MemoryResult = Result<i64, StorageError>;
type MemoryReply = SyncSender<MemoryResult>;

enum Cmd {
    Event(Box<EventRecord>),
    Memory {
        content: String,
        rec: Box<MemoryRecord>,
        reply: SyncSender<Result<i64, StorageError>>,
    },
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
        let content = rec.sanitize()?;
        let (reply, wait) = sync_channel(1);
        let tx = self.tx.as_ref().ok_or(StorageError::WriterUnavailable)?;
        tx.try_send(Cmd::Memory {
            content,
            rec: Box::new(rec),
            reply,
        })
        .map_err(|_| StorageError::WriterUnavailable)?;
        wait.recv_timeout(timeout)
            .map_err(|_| StorageError::WriterUnavailable)?
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
        let mut replies: Vec<(MemoryReply, MemoryResult)> = Vec::new();
        let mut events_written = 0u64;
        match conn.transaction() {
            Err(_) => {
                failed.fetch_add(batch.len() as u64, Ordering::Relaxed);
                for c in batch {
                    match c {
                        Cmd::Memory { reply, .. } => {
                            let _ = reply.send(Err(StorageError::WriterUnavailable));
                        }
                        Cmd::Flush(r) => {
                            let _ = r.send(());
                        }
                        Cmd::Event(_) => {}
                    }
                }
                continue;
            }
            Ok(tx) => {
                for c in batch {
                    match c {
                        Cmd::Event(rec) => match insert_event(&tx, &rec) {
                            Ok(n) => events_written += n,
                            Err(_) => {
                                failed.fetch_add(1, Ordering::Relaxed);
                            }
                        },
                        Cmd::Memory {
                            content,
                            rec,
                            reply,
                        } => {
                            replies.push((reply, insert_memory(&tx, &rec, &content)));
                        }
                        Cmd::Flush(r) => flushes.push(r),
                    }
                }
                if tx.commit().is_err() {
                    failed.fetch_add(1, Ordering::Relaxed);
                    replies
                        .iter_mut()
                        .for_each(|(_, r)| *r = Err(StorageError::WriterUnavailable));
                }
            }
        }
        for (reply, res) in replies {
            let _ = reply.send(res);
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

fn insert_memory(
    tx: &Transaction<'_>,
    rec: &MemoryRecord,
    content: &str,
) -> Result<i64, StorageError> {
    tx.execute(
        "INSERT INTO memories (project_id, content, source) VALUES (?1, ?2, ?3)",
        rusqlite::params![rec.project_id, content, rec.source],
    )?;
    Ok(tx.last_insert_rowid())
}
