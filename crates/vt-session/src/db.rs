//! Shared database access for the main application database.
//!
//! Every in-process write goes through ONE connection per database file (the
//! [`Writer`]), borrowed exclusively for the length of a `BEGIN IMMEDIATE`
//! transaction. Reads use separate short-lived `query_only` connections, so a
//! read never waits for, and is never blocked by, an in-progress write (WAL).
//! The recorder (vt-record) keeps its own durable connection and still
//! contends with the writer at the SQLite level; the writer's busy timeout and
//! the recorder's manifest retry cover that.
//!
//! Rules (enforced at runtime where possible):
//! - A `write`/`read` closure only touches the database. No file system,
//!   Keychain, network, sleeps or channel waits inside it.
//! - No nested access: calling `write` or `read` from inside a `write`/`read`
//!   closure on the same thread fails at once with [`NestedAccess`]. Pass the
//!   open transaction (`&Connection`) to helpers instead. (A nested write would
//!   otherwise deadlock on the writer; a nested read on another connection
//!   cannot see the closure's uncommitted rows.)
//! - Lock order: database access is the innermost lock. Code may call
//!   `write`/`read` while holding `AppService::runtime` or `backlog`, but must
//!   never acquire those (or any [`OrderedMutex`]) inside a `write`/`read`
//!   closure. [`OrderedMutex::lock`] asserts this in debug builds. While a
//!   thread holds an `OrderedMutex` guard, `write` waits at most
//!   [`HELD_LOCK_WRITE_WAIT`] (5 s, as before this change), and an explicit
//!   longer `write_with` panics in debug builds.
//! - Fairness: waiters get the writer in arrival (FIFO) order.
//! - Identity: the writer is looked up by the canonicalized database path, so
//!   `/var/...` and `/private/var/...` share one writer. While any handle is
//!   alive (the `AppService` holds one for its lifetime) every lookup returns
//!   the same writer.

use anyhow::{Context, Result};
use rusqlite::{Connection, OpenFlags, Transaction, TransactionBehavior};
use std::{
    cell::Cell,
    collections::{HashMap, VecDeque},
    fmt,
    path::{Path, PathBuf},
    sync::{
        Arc, Condvar, LockResult, Mutex, MutexGuard, OnceLock, PoisonError, Weak,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

pub(crate) const DB_FILE: &str = "vantranscribe.db";
/// Default wait for the shared writer, and the writer connection's SQLite busy
/// timeout. Generous: the writer only contends with the recorder's own
/// connection (and test lock holders); in-process writers queue on the mutex.
pub(crate) const WRITE_WAIT: Duration = Duration::from_secs(30);
/// Busy timeout of read connections (WAL readers rarely wait at all).
pub(crate) const READ_BUSY_TIMEOUT: Duration = Duration::from_secs(5);
/// Debug builds log any write transaction that holds the writer longer.
pub(crate) const SLOW_WRITE: Duration = Duration::from_millis(250);

/// Returned when `write` or `read` is called from inside a `write`/`read`
/// closure on the same thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NestedAccess {
    pub outer: &'static str,
    pub inner: &'static str,
}
impl fmt::Display for NestedAccess {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "nested database {} inside a database {} closure on the same thread; pass the open transaction to the helper instead",
            self.inner, self.outer
        )
    }
}
impl std::error::Error for NestedAccess {}

thread_local! {
    static SCOPE: Cell<Option<&'static str>> = const { Cell::new(None) };
}

struct ScopeGuard;
impl ScopeGuard {
    fn enter(kind: &'static str) -> Result<Self> {
        SCOPE.with(|scope| match scope.get() {
            Some(outer) => Err(anyhow::Error::new(NestedAccess { outer, inner: kind })),
            None => {
                scope.set(Some(kind));
                Ok(ScopeGuard)
            }
        })
    }
}
impl Drop for ScopeGuard {
    fn drop(&mut self) {
        SCOPE.with(|scope| scope.set(None));
    }
}

/// True while this thread is inside a `write` or `read` closure.
pub(crate) fn in_db_closure() -> Option<&'static str> {
    SCOPE.with(Cell::get)
}

/// Lock-order rule check: application locks are never taken inside a
/// database closure (the database is the innermost lock).
#[track_caller]
pub(crate) fn assert_outside_db_closure(lock: &str) {
    if cfg!(debug_assertions)
        && let Some(kind) = in_db_closure()
    {
        panic!(
            "lock-order violation: `{lock}` acquired inside a database {kind} closure (database access must be the innermost lock)"
        );
    }
}

/// Longest wait a write may use while this thread holds an [`OrderedMutex`]
/// (`runtime`/`backlog`): HEAD's 5 s busy timeout. The capture loop takes
/// `runtime`, so a longer wait under it could stall capture.
pub(crate) const HELD_LOCK_WRITE_WAIT: Duration = Duration::from_secs(5);

thread_local! {
    static HELD: Cell<u32> = const { Cell::new(0) };
}

/// In-process queue wait for writes made on the capture consumer loop. Its
/// housekeeping writes are skippable (`tolerate_busy`), and the loop also
/// feeds the recorder, so losing the queue must cost a few ms, not the whole
/// `LOOP_BUSY_TIMEOUT`. The SQLite lock wait keeps the caller's budget (as
/// before this change, the recorder's own lock can still cost up to it).
pub(crate) const CAPTURE_LOOP_QUEUE_WAIT: Duration = Duration::from_millis(5);

thread_local! {
    static CAPTURE_LOOP: Cell<bool> = const { Cell::new(false) };
}

/// Marks the current thread as the capture consumer loop while alive.
pub(crate) struct CaptureLoopScope(());
impl CaptureLoopScope {
    pub(crate) fn enter() -> Self {
        CAPTURE_LOOP.with(|flag| flag.set(true));
        Self(())
    }
}
impl Drop for CaptureLoopScope {
    fn drop(&mut self) {
        CAPTURE_LOOP.with(|flag| flag.set(false));
    }
}

/// Number of [`OrderedMutex`] guards this thread currently holds.
pub(crate) fn held_app_locks() -> u32 {
    HELD.with(Cell::get)
}

/// A `Mutex` for application state that participates in the database lock
/// order (`AppService::runtime`, `AppService::backlog`). Same API shape as
/// `std::sync::Mutex::lock`, plus debug checks of the lock-order rule and a
/// per-thread count of held guards (writes under a guard must use a wait of
/// at most [`HELD_LOCK_WRITE_WAIT`]).
pub(crate) struct OrderedMutex<T> {
    name: &'static str,
    inner: Mutex<T>,
}
pub(crate) struct OrderedGuard<'a, T> {
    inner: MutexGuard<'a, T>,
}
impl<'a, T> OrderedGuard<'a, T> {
    fn new(inner: MutexGuard<'a, T>) -> Self {
        HELD.with(|held| held.set(held.get() + 1));
        Self { inner }
    }
}
impl<T> Drop for OrderedGuard<'_, T> {
    fn drop(&mut self) {
        HELD.with(|held| held.set(held.get().saturating_sub(1)));
    }
}
impl<T> std::ops::Deref for OrderedGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.inner
    }
}
impl<T> std::ops::DerefMut for OrderedGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.inner
    }
}
impl<T> OrderedMutex<T> {
    pub(crate) fn new(name: &'static str, value: T) -> Self {
        Self {
            name,
            inner: Mutex::new(value),
        }
    }
    #[track_caller]
    pub(crate) fn lock(&self) -> LockResult<OrderedGuard<'_, T>> {
        assert_outside_db_closure(self.name);
        match self.inner.lock() {
            Ok(guard) => Ok(OrderedGuard::new(guard)),
            Err(poisoned) => Err(PoisonError::new(OrderedGuard::new(poisoned.into_inner()))),
        }
    }
}

static LONGEST_WRITE_NS: AtomicU64 = AtomicU64::new(0);

/// Longest write closure (transaction body plus COMMIT, excluding any wait
/// for another connection's lock) in this process. For tests and diagnostics.
#[allow(dead_code)]
pub(crate) fn longest_write() -> Duration {
    Duration::from_nanos(LONGEST_WRITE_NS.load(Ordering::Relaxed))
}

/// The database file for `root`, with the directory canonicalized so that
/// every spelling of one folder (symlinks, `/var` vs `/private/var`) maps to
/// one writer.
pub(crate) fn database_path(root: &Path) -> Result<PathBuf> {
    Ok(std::fs::canonicalize(root)
        .with_context(|| format!("Recordings folder unavailable: {}", root.display()))?
        .join(DB_FILE))
}

fn registry() -> &'static Mutex<HashMap<PathBuf, Weak<Writer>>> {
    static WRITERS: OnceLock<Mutex<HashMap<PathBuf, Weak<Writer>>>> = OnceLock::new();
    WRITERS.get_or_init(Default::default)
}

/// The single write connection of one database file.
pub(crate) struct Writer {
    path: PathBuf,
    slot: Mutex<Slot>,
    returned: Condvar,
}

/// The connection when free, plus the FIFO of waiting tickets. The
/// connection is handed out strictly in arrival order: only the ticket at the
/// head of the queue may take it.
struct Slot {
    /// `None` while leased, or after a connection was discarded because it
    /// could not be returned to autocommit (the next holder reopens it).
    db: Option<Connection>,
    leased: bool,
    queue: VecDeque<u64>,
    next_ticket: u64,
}

impl fmt::Debug for Writer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Writer").field("path", &self.path).finish()
    }
}

impl Writer {
    /// The live writer for `root`'s database, opening it if none is alive.
    pub(crate) fn for_root(root: &Path) -> Result<Arc<Writer>> {
        let path = database_path(root)?;
        let live = |path: &PathBuf| {
            registry()
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(path)
                .and_then(Weak::upgrade)
        };
        if let Some(writer) = live(&path) {
            return Ok(writer);
        }
        // Open outside the registry lock (opening can wait on the file), then
        // publish it unless another thread published a live writer first; a
        // losing candidate is dropped unused, so only one writer ever writes.
        let candidate = Arc::new(Writer::open(path.clone())?);
        let mut writers = registry().lock().unwrap_or_else(|e| e.into_inner());
        if let Some(writer) = writers.get(&path).and_then(Weak::upgrade) {
            return Ok(writer);
        }
        writers.retain(|_, writer| writer.strong_count() > 0);
        writers.insert(path, Arc::downgrade(&candidate));
        Ok(candidate)
    }

    fn open(path: PathBuf) -> Result<Self> {
        let db = Self::connect(&path)?;
        Ok(Self {
            path,
            slot: Mutex::new(Slot {
                db: Some(db),
                leased: false,
                queue: VecDeque::new(),
                next_ticket: 0,
            }),
            returned: Condvar::new(),
        })
    }

    /// Opens the write connection: WAL verified, foreign keys on.
    fn connect(path: &Path) -> Result<Connection> {
        let db = Connection::open(path)?;
        db.busy_timeout(WRITE_WAIT)?;
        let mode: String = db.query_row("PRAGMA journal_mode=WAL", [], |r| r.get(0))?;
        if !mode.eq_ignore_ascii_case("wal") {
            anyhow::bail!("The database could not enter WAL mode (journal_mode={mode}).");
        }
        db.pragma_update(None, "foreign_keys", "ON")?;
        Ok(db)
    }

    #[allow(dead_code)]
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// One `BEGIN IMMEDIATE` transaction on the shared connection, committed
    /// when `f` returns `Ok` and rolled back otherwise. `wait` bounds the total
    /// wait: first for the writer, then (with what remains) for SQLite's write
    /// lock held by another connection (the recorder). Either expiry is
    /// reported as SQLITE_BUSY so `store::sqlite_busy` classifies it as lock
    /// contention, exactly as a busy-timeout expiry was before.
    #[track_caller]
    pub(crate) fn write_with<T>(
        &self,
        wait: Duration,
        f: impl FnOnce(&Transaction<'_>) -> Result<T>,
    ) -> Result<T> {
        let caller = std::panic::Location::caller();
        if cfg!(debug_assertions) && held_app_locks() > 0 && wait > HELD_LOCK_WRITE_WAIT {
            panic!(
                "lock-order violation: database write waiting up to {wait:?} while holding `runtime`/`backlog` at {}:{} (use write_with with at most {HELD_LOCK_WRITE_WAIT:?}, or write before taking the lock)",
                caller.file(),
                caller.line()
            );
        }
        let _scope = ScopeGuard::enter("write")?;
        let waiting = Instant::now();
        let queue_wait = if CAPTURE_LOOP.with(Cell::get) {
            wait.min(CAPTURE_LOOP_QUEUE_WAIT)
        } else {
            wait
        };
        let mut lease = self.checkout(queue_wait)?;
        let db = lease.connection();
        db.busy_timeout(wait.saturating_sub(waiting.elapsed()))?;
        let leased = Instant::now();
        let mut began = None;
        let result = (|| {
            let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            began = Some(Instant::now());
            let value = f(&tx)?;
            tx.commit()?;
            Ok(value)
        })();
        // `closure` is the transaction body plus COMMIT, the part this code
        // controls; `lock_wait` is BEGIN IMMEDIATE waiting for another
        // connection (the recorder). Both keep other in-process writers queued.
        let lock_wait = began.map_or(leased.elapsed(), |began| began - leased);
        let closure = began.map_or(Duration::ZERO, |began| began.elapsed());
        LONGEST_WRITE_NS.fetch_max(
            closure.as_nanos().min(u64::MAX as u128) as u64,
            Ordering::Relaxed,
        );
        if cfg!(debug_assertions) && closure + lock_wait > SLOW_WRITE {
            eprintln!(
                "slow database write: {:?} holding the shared writer ({closure:?} in the closure and commit, {lock_wait:?} waiting for another connection's lock) at {}:{}",
                closure + lock_wait,
                caller.file(),
                caller.line()
            );
        }
        result
    }

    /// Waits up to `wait` for the connection, served in arrival order. A
    /// waiter that times out leaves the queue and wakes the others, so the
    /// next in line is never stranded behind it.
    fn checkout(&self, wait: Duration) -> Result<Lease<'_>> {
        let deadline = Instant::now() + wait;
        let mut slot = self.slot.lock().unwrap_or_else(|e| e.into_inner());
        let ticket = slot.next_ticket;
        slot.next_ticket += 1;
        slot.queue.push_back(ticket);
        loop {
            if slot.queue.front() == Some(&ticket) && !slot.leased {
                slot.queue.pop_front();
                let db = match slot.db.take() {
                    Some(db) => Ok(db),
                    // The previous connection was discarded: reopen it.
                    None => {
                        #[cfg(test)]
                        REOPENED.fetch_add(1, Ordering::Relaxed);
                        Self::connect(&self.path)
                    }
                };
                return match db {
                    Ok(db) => {
                        slot.leased = true;
                        Ok(Lease {
                            writer: self,
                            db: Some(db),
                        })
                    }
                    Err(error) => {
                        // Let the next waiter try to reopen.
                        self.returned.notify_all();
                        Err(error)
                    }
                };
            }
            let now = Instant::now();
            if now >= deadline {
                slot.queue.retain(|queued| *queued != ticket);
                // If this waiter was the head, the new head may take a free
                // connection now.
                self.returned.notify_all();
                return Err(writer_busy(wait));
            }
            slot = self
                .returned
                .wait_timeout(slot, deadline - now)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }

    /// Callers currently queued for the connection (tests).
    #[cfg(test)]
    pub(crate) fn waiting(&self) -> usize {
        self.slot
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .queue
            .len()
    }
}

struct Lease<'a> {
    writer: &'a Writer,
    db: Option<Connection>,
}
impl Lease<'_> {
    fn connection(&mut self) -> &mut Connection {
        self.db.as_mut().expect("leased connection")
    }
}
impl Drop for Lease<'_> {
    fn drop(&mut self) {
        if let Some(db) = self.db.take() {
            let db = reset(db);
            let mut slot = self.writer.slot.lock().unwrap_or_else(|e| e.into_inner());
            slot.db = db;
            slot.leased = false;
            drop(slot);
            // Every waiter re-checks; only the head of the queue proceeds.
            self.writer.returned.notify_all();
        }
    }
}

#[cfg(test)]
thread_local! {
    /// Test hook: make the next returned connection look like a failed
    /// ROLLBACK left it inside a transaction.
    pub(crate) static SIMULATE_FAILED_ROLLBACK: Cell<bool> = const { Cell::new(false) };
}
#[cfg(test)]
pub(crate) static REOPENED: AtomicU64 = AtomicU64::new(0);

/// Returns the connection to autocommit. If that fails it is discarded
/// (closing it rolls back), so one failed ROLLBACK cannot leave the only
/// writer stuck in "cannot start a transaction within a transaction".
fn reset(db: Connection) -> Option<Connection> {
    #[cfg(test)]
    let simulate = SIMULATE_FAILED_ROLLBACK.with(|flag| flag.replace(false));
    #[cfg(not(test))]
    let simulate = false;
    if simulate {
        let _ = db.execute_batch("BEGIN");
    } else if !db.is_autocommit() {
        let _ = db.execute_batch("ROLLBACK");
    }
    if db.is_autocommit() {
        Some(db)
    } else {
        eprintln!("database writer: ROLLBACK failed; discarding the connection to reopen it");
        None
    }
}

fn writer_busy(wait: Duration) -> anyhow::Error {
    anyhow::Error::new(rusqlite::Error::SqliteFailure(
        rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_BUSY),
        Some(format!(
            "database is locked: the shared database writer stayed busy for {wait:?}"
        )),
    ))
}

/// A write transaction on `root`'s shared writer with the default wait:
/// [`WRITE_WAIT`], or [`HELD_LOCK_WRITE_WAIT`] (HEAD's 5 s) when this thread
/// holds `runtime`/`backlog`, so a write queued behind the recorder can never
/// stall the capture loop (which takes `runtime`) for longer than before.
#[track_caller]
pub(crate) fn write<T>(root: &Path, f: impl FnOnce(&Transaction<'_>) -> Result<T>) -> Result<T> {
    write_with(root, default_wait(), f)
}

/// The wait `write` uses on this thread right now.
pub(crate) fn default_wait() -> Duration {
    if held_app_locks() > 0 {
        HELD_LOCK_WRITE_WAIT
    } else {
        WRITE_WAIT
    }
}

/// A write transaction on `root`'s shared writer. `wait` bounds the total
/// wait: the in-process queue for the writer first, then what remains for
/// SQLite's lock held by another connection (the recorder). On the capture
/// loop ([`CaptureLoopScope`]) the queue part is at most
/// [`CAPTURE_LOOP_QUEUE_WAIT`].
#[track_caller]
pub(crate) fn write_with<T>(
    root: &Path,
    wait: Duration,
    f: impl FnOnce(&Transaction<'_>) -> Result<T>,
) -> Result<T> {
    if let Some(outer) = in_db_closure() {
        // Fail before the registry lookup so the error is the nesting error.
        return Err(anyhow::Error::new(NestedAccess {
            outer,
            inner: "write",
        }));
    }
    Writer::for_root(root)?.write_with(wait, f)
}

/// A read on a separate `query_only` connection inside one read transaction
/// (a consistent snapshot). Never waits for the writer; sees only committed
/// data.
pub(crate) fn read<T>(root: &Path, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
    let _scope = ScopeGuard::enter("read")?;
    let db = open_reader(root)?;
    let tx = db.unchecked_transaction()?;
    let value = f(&tx)?;
    tx.commit()?;
    Ok(value)
}

fn open_reader(root: &Path) -> Result<Connection> {
    let db = Connection::open_with_flags(
        database_path(root)?,
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_URI,
    )?;
    db.busy_timeout(READ_BUSY_TIMEOUT)?;
    db.pragma_update(None, "query_only", "ON")?;
    Ok(db)
}

#[cfg(test)]
#[path = "db_tests.rs"]
mod tests;
