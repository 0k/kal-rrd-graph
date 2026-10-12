//! Displayed-graph interests, directory watches and the graph-change channel.
//!
//! Reads application-supplied graph definitions but never owns
//! configuration reloads or reads/writes RRD data. One worker owns all watches.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::{mpsc, Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use kal_fs_watch::notify::{
    self,
    event::{AccessKind, AccessMode, ModifyKind},
    Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher,
};
use kal_sse::core::{EventLog, SubId};
use serde::Serialize;

use crate::pattern::{FilePattern, PatternMatch};
use crate::source::{GraphSource, Lookup};

const SUBSCRIPTION_TTL: Duration = Duration::from_secs(15 * 60);
const MAX_EVENTS: usize = 1024;
const CHANNEL_CAPACITY: usize = 256;
const SETTLE: Duration = Duration::from_secs(1);
const GRACE: Duration = Duration::from_secs(30);
const TICK: Duration = Duration::from_secs(1);

/// Initialization, subscription validation or worker communication failure.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum WatchError {
    #[error("graph watch timing must be nonzero")]
    NonzeroTiming,
    #[error("failed to initialize graph file watcher (inotify): {0}")]
    Initialize(#[source] notify::Error),
    #[error("failed to spawn graph watch worker: {0}")]
    Spawn(#[source] std::io::Error),
    #[error("invalid subscription id")]
    InvalidSubscription,
    #[error("{id:?} is a group, not a graph")]
    NotAGraph { id: String },
    #[error("no graph {id:?}")]
    NotFound { id: String },
    #[error("graph watch worker stopped: {0}")]
    WorkerStopped(String),
    #[error("graph watch rescan failed: {0}")]
    Rescan(#[source] mpsc::RecvError),
}

/// The graph whose files changed; clients fetch that graph's tail.
#[derive(Debug, Serialize, Clone)]
pub struct GraphChange {
    pub id: String,
}

/// Snapshot of one client's displayed graph ids, sorted and deduplicated.
#[derive(Debug, Serialize, Clone)]
pub struct WatchSnapshot {
    pub ids: Vec<String>,
}

/// Worker timing, injectable so lifecycle and debounce tests need no long waits.
#[derive(Debug, Clone, Copy)]
pub struct WatchTiming {
    pub settle: Duration,
    pub grace: Duration,
    pub tick: Duration,
}

impl Default for WatchTiming {
    fn default() -> Self {
        Self {
            settle: SETTLE,
            grace: GRACE,
            tick: TICK,
        }
    }
}

struct Interest {
    ids: BTreeSet<String>,
    streams: usize,
    idle_since: Option<Instant>,
}

enum Message {
    Fs(notify::Result<Event>),
    Rescan(Option<mpsc::Sender<()>>),
    Stop,
}

/// Single owner of the union of displayed graphs and its file-change log.
pub struct GraphWatch {
    live: Arc<dyn GraphSource>,
    interests: Arc<Mutex<HashMap<SubId, Interest>>>,
    events: Arc<Mutex<EventLog<GraphChange>>>,
    tx: mpsc::Sender<Message>,
    worker: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for GraphWatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GraphWatch")
            .field("worker", &self.worker)
            .finish_non_exhaustive()
    }
}

impl GraphWatch {
    /// Initialize the notify backend before serving either run mode.
    pub fn new(live: Arc<dyn GraphSource>) -> Result<Arc<Self>, WatchError> {
        Self::with_timing(live, WatchTiming::default())
    }

    /// As `new`, with bounded, nonzero worker timing for integration tests.
    pub fn with_timing(
        live: Arc<dyn GraphSource>,
        timing: WatchTiming,
    ) -> Result<Arc<Self>, WatchError> {
        if timing.tick.is_zero() || timing.settle.is_zero() || timing.grace.is_zero() {
            return Err(WatchError::NonzeroTiming);
        }
        let (tx, rx) = mpsc::channel();
        let callback_tx = tx.clone();
        let watcher = notify::recommended_watcher(move |event| {
            // Receiver shutdown is normal: the worker owns and drops this watcher.
            let _ = callback_tx.send(Message::Fs(event));
        })
        .map_err(WatchError::Initialize)?;
        let interests = Arc::new(Mutex::new(HashMap::new()));
        let events = Arc::new(Mutex::new(EventLog::new(
            SUBSCRIPTION_TTL,
            MAX_EVENTS,
            CHANNEL_CAPACITY,
        )));
        let worker_live = live.clone();
        let worker_interests = interests.clone();
        let worker_events = events.clone();
        let worker = thread::Builder::new()
            .name("rrd-graph-watch".into())
            .spawn(move || {
                Worker {
                    watcher,
                    live: worker_live,
                    interests: worker_interests,
                    events: worker_events,
                    timing,
                    dirs: BTreeSet::new(),
                    plan: BTreeMap::new(),
                    pending: BTreeMap::new(),
                    revision: 0,
                }
                .run(rx);
            })
            .map_err(WatchError::Spawn)?;
        Ok(Arc::new(Self {
            live,
            interests,
            events,
            tx,
            worker: Some(worker),
        }))
    }

    /// Register or replace one interest after validating the current config.
    pub fn subscribe(
        &self,
        sub: Option<String>,
        ids: Vec<String>,
    ) -> Result<(WatchSnapshot, SubId, u64), WatchError> {
        if sub.as_deref().is_some_and(|sub| !valid_sub(sub)) {
            return Err(WatchError::InvalidSubscription);
        }
        let ids: BTreeSet<_> = ids.into_iter().collect();
        let ordered: Vec<_> = ids.iter().cloned().collect();
        for (id, lookup) in ordered.iter().zip(self.live.lookup_many(&ordered)) {
            match lookup {
                Lookup::Graph(_) => {}
                Lookup::NotAGraph => return Err(WatchError::NotAGraph { id: id.clone() }),
                Lookup::NotFound => return Err(WatchError::NotFound { id: id.clone() }),
            }
        }
        let (sub, index) = {
            let mut interests = self
                .interests
                .lock()
                .expect("graph interests mutex poisoned");
            let (sub, index) = self
                .events
                .lock()
                .expect("graph event log mutex poisoned")
                .register(sub);
            let streams = interests.get(&sub).map_or(0, |interest| interest.streams);
            interests.insert(
                sub.clone(),
                Interest {
                    ids: ids.clone(),
                    streams,
                    idle_since: (streams == 0).then(Instant::now),
                },
            );
            (sub, index)
        };
        // A snapshot promises these interests are active. Wait for the worker's
        // rescan, rather than losing an update immediately after HTTP returns.
        let (ready_tx, ready_rx) = mpsc::channel();
        self.tx
            .send(Message::Rescan(Some(ready_tx)))
            .map_err(|e| WatchError::WorkerStopped(e.to_string()))?;
        ready_rx.recv().map_err(WatchError::Rescan)?;
        Ok((
            WatchSnapshot {
                ids: ids.into_iter().collect(),
            },
            sub,
            index,
        ))
    }

    #[cfg(feature = "api")]
    pub(crate) fn log(&self) -> Arc<Mutex<EventLog<GraphChange>>> {
        self.events.clone()
    }

    /// Keep the interest alive for the lifetime of a successful SSE body.
    #[cfg(feature = "api")]
    pub(crate) fn attach(self: &Arc<Self>, sub: &str) -> Option<StreamGuard> {
        let mut interests = self
            .interests
            .lock()
            .expect("graph interests mutex poisoned");
        let interest = interests.get_mut(sub)?;
        interest.streams += 1;
        interest.idle_since = None;
        Some(StreamGuard {
            watch: self.clone(),
            sub: sub.to_string(),
        })
    }
}

impl Drop for GraphWatch {
    fn drop(&mut self) {
        if let Err(error) = self.tx.send(Message::Stop) {
            log::warn!("graph watch worker already stopped: {error}");
        }
        if let Some(worker) = self.worker.take() {
            if worker.join().is_err() {
                log::warn!("graph watch worker panicked during shutdown");
            }
        }
    }
}

#[cfg(feature = "api")]
pub(crate) struct StreamGuard {
    watch: Arc<GraphWatch>,
    sub: SubId,
}

#[cfg(feature = "api")]
impl Drop for StreamGuard {
    fn drop(&mut self) {
        {
            let mut interests = self
                .watch
                .interests
                .lock()
                .expect("graph interests mutex poisoned");
            if let Some(interest) = interests.get_mut(&self.sub) {
                interest.streams -= 1;
                if interest.streams == 0 {
                    interest.idle_since = Some(Instant::now());
                }
            }
        }
        if let Err(error) = self.watch.tx.send(Message::Rescan(None)) {
            log::warn!("cannot rescan graph watches on disconnect: {error}");
        }
    }
}

pub(crate) fn valid_sub(sub: &str) -> bool {
    sub.len() == 36
        && sub.bytes().enumerate().all(|(i, byte)| {
            if matches!(i, 8 | 13 | 18 | 23) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        })
}

struct Plan {
    root: PathBuf,
    files: FilePattern,
}

struct Worker {
    watcher: RecommendedWatcher,
    live: Arc<dyn GraphSource>,
    interests: Arc<Mutex<HashMap<SubId, Interest>>>,
    events: Arc<Mutex<EventLog<GraphChange>>>,
    timing: WatchTiming,
    dirs: BTreeSet<PathBuf>,
    plan: BTreeMap<String, Plan>,
    pending: BTreeMap<String, Instant>,
    revision: u64,
}

impl Worker {
    fn run(mut self, rx: mpsc::Receiver<Message>) {
        let mut tick = Instant::now();
        loop {
            let now = Instant::now();
            let timeout = self
                .pending
                .values()
                .map(|last| self.timing.settle.saturating_sub(now.duration_since(*last)))
                .fold(tick.saturating_duration_since(now), Duration::min);
            match rx.recv_timeout(timeout) {
                Ok(Message::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => return,
                Ok(Message::Rescan(ready)) => {
                    self.rescan();
                    if let Some(ready) = ready {
                        if let Err(error) = ready.send(()) {
                            log::debug!("graph snapshot caller disconnected: {error}");
                        }
                    }
                }
                Ok(Message::Fs(Ok(event))) => {
                    if self.on_event(event) {
                        self.rescan();
                    }
                }
                Ok(Message::Fs(Err(error))) => log::warn!("graph watch notify error: {error}"),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
            let now = Instant::now();
            if now >= tick {
                let expired = self.expire(now);
                if expired || self.live.revision() != self.revision {
                    self.rescan();
                }
                self.events
                    .lock()
                    .expect("graph event log mutex poisoned")
                    .compact(now);
                tick = now + self.timing.tick;
            }
            let ready: Vec<_> = self
                .pending
                .iter()
                .filter(|(_, last)| now.duration_since(**last) >= self.timing.settle)
                .map(|(id, _)| id.clone())
                .collect();
            for id in ready {
                self.pending.remove(&id);
                if self.plan.contains_key(&id) {
                    self.events
                        .lock()
                        .expect("graph event log mutex poisoned")
                        .append(GraphChange { id });
                }
            }
        }
    }

    fn expire(&self, now: Instant) -> bool {
        let mut interests = self
            .interests
            .lock()
            .expect("graph interests mutex poisoned");
        let mut expired = false;
        interests.retain(|sub, interest| {
            if interest
                .idle_since
                .is_some_and(|idle| now.duration_since(idle) >= self.timing.grace)
            {
                // @0k/kal-sse-client always re-snapshots (same sub, redeclared
                // ids) before reconnecting and tail-fetches every graph then.
                // Grace only covers quick reconnects; dropping watches loses no data.
                // Also start the log TTL for never-attached/dropped bodies: the
                // SSE adapter cannot execute its final disconnect on cancellation.
                let mut events = self.events.lock().expect("graph event log mutex poisoned");
                match events.attach(sub) {
                    Ok(replay) => events.disconnect(sub, replay.epoch, now),
                    Err(error) => log::debug!("expired graph interest {sub}: {error}"),
                }
                expired = true;
                false
            } else {
                true
            }
        });
        expired
    }

    fn rescan(&mut self) {
        let ids: BTreeSet<_> = self
            .interests
            .lock()
            .expect("graph interests mutex poisoned")
            .values()
            .flat_map(|interest| interest.ids.iter().cloned())
            .collect();
        // Read the revision before the snapshot: a concurrent newer reload is
        // detected on the next tick, never mistaken for an already-applied one.
        let revision = self.live.revision();
        let ids: Vec<_> = ids.into_iter().collect();
        let lookups = self.live.lookup_many(&ids);
        let plan: BTreeMap<_, _> = ids
            .into_iter()
            .zip(lookups)
            .filter_map(|(id, lookup)| {
                let Lookup::Graph(graph) = lookup else {
                    return None;
                };
                Some((
                    id,
                    Plan {
                        root: graph.rrd_dir.clone(),
                        files: graph.files.clone(),
                    },
                ))
            })
            .collect();
        let dirs: BTreeSet<_> = plan
            .values()
            .flat_map(|graph| graph.files.dirs(&graph.root))
            .collect();
        let old_counts = (self.plan.len(), self.dirs.len());
        let removed: Vec<_> = self.dirs.difference(&dirs).cloned().collect();
        for dir in removed {
            if let Err(error) = self.watcher.unwatch(&dir) {
                log::warn!("cannot unwatch {}: {error}", dir.display());
            }
            self.dirs.remove(&dir);
        }
        for dir in dirs.difference(&self.dirs).cloned().collect::<Vec<_>>() {
            match self.watcher.watch(&dir, RecursiveMode::NonRecursive) {
                Ok(()) => {
                    self.dirs.insert(dir);
                }
                Err(error) => log::warn!("cannot watch {}: {error}", dir.display()),
            }
        }
        self.plan = plan;
        self.pending.retain(|id, _| self.plan.contains_key(id));
        self.revision = revision;
        if old_counts != (self.plan.len(), self.dirs.len()) {
            log::info!(
                "watching {} graph(s) in {} directorie(s)",
                self.plan.len(),
                self.dirs.len()
            );
        }
    }

    fn on_event(&mut self, event: Event) -> bool {
        // librrd updates through mmap: no IN_MODIFY, only IN_OPEN and
        // IN_CLOSE_WRITE (verified with inotifywait). Fetch/info reads produce
        // IN_CLOSE_NOWRITE, so accepting close-write cannot cause a read loop.
        // rsync's hidden temp then rename is handled by Create/Modify(Name);
        // FilePattern's stars never match the hidden temporary name.
        if !kal_fs_watch::is_write_event(&event.kind)
            && !matches!(
                event.kind,
                EventKind::Access(AccessKind::Close(AccessMode::Write))
            )
        {
            return false;
        }
        let structural = matches!(
            event.kind,
            EventKind::Create(_) | EventKind::Remove(_) | EventKind::Modify(ModifyKind::Name(_))
        );
        let mut rescan = false;
        for path in &event.paths {
            for (id, graph) in &self.plan {
                let Ok(rel) = path.strip_prefix(&graph.root) else {
                    continue;
                };
                match graph.files.match_rel(rel) {
                    PatternMatch::Full => {}
                    PatternMatch::Prefix if structural => {
                        rescan = true;
                    }
                    _ => continue,
                }
                log::debug!("graph {id} changed: {:?} {}", event.kind, path.display());
                self.pending.insert(id.clone(), Instant::now());
            }
        }
        rescan
    }
}
