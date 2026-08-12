// TODO(ai-review): review for style and correctness
//! Central chunk service. Every CDN chunk fetch funnels through here:
//! bulk downloads ([`ChunkService::enqueue`]), interactive reads
//! ([`ChunkService::enqueue_and_wait`]), and implicit reads arriving via
//! [`crate::steam::chunk_store::TrackedChunkStore`].
//!
//! Provides single-flight per SHA, interactive-over-bulk priority, and a
//! single accounting point so download totals stay fixed after enqueue.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use bytes::Bytes;
use serde::Serialize;
#[allow(unused_imports)]
use serde_json::json;
use steam_depot_vfs::chunk_store::ChunkStore;
use steam_depot_vfs::{ChunkHash, VfsError};
use tokio::sync::{Semaphore, broadcast, mpsc, oneshot, watch};
use utoipa::ToSchema;

use super::Snapshot;
use super::store_index::StoreIndex;
use crate::steam::{DepotId, ManifestId};

/// Coalesce window for the per-file `chunks` SSE events, so a busy
/// download emits one event per (manifest, file) instead of per chunk.
const CHUNK_FLUSH_INTERVAL: Duration = Duration::from_millis(100);

/// Empirically the prefetch CLI saturates at 32 parallel CDN fetches. Be
/// a bit gentler in the viewer because we share the connection with
/// foreground requests (manifest fetches, etc.).
const DEFAULT_PARALLELISM: usize = 16;

fn parallelism() -> usize {
    std::env::var("DOWNLOAD_PARALLELISM")
        .ok()
        .and_then(|s| s.parse().ok())
        .filter(|&n: &usize| n > 0)
        .unwrap_or(DEFAULT_PARALLELISM)
}

/// Optional per-chunk delay in ms, for observing progress UI. Debug-only.
#[cfg(debug_assertions)]
fn throttle_ms() -> u64 {
    std::env::var("DOWNLOAD_THROTTLE_MS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

#[cfg(not(debug_assertions))]
fn throttle_ms() -> u64 {
    0
}

#[derive(Clone, Copy)]
enum Priority {
    Interactive,
    Bulk,
}

pub struct ChunkService {
    interactive: mpsc::UnboundedSender<Job>,
    bulk: mpsc::UnboundedSender<Job>,
    stats: Mutex<DownloadStats>,
    events: broadcast::Sender<DownloadEvent>,
    /// Chunks enqueued but not yet settled. Membership decides the
    /// accounting regime in [`Self::fetch_via`]: pending SHAs were counted
    /// at enqueue, everything else is an implicit read counted on the fly.
    /// Doubles as the enqueue-dedup set and carries completion waiters.
    pending: Mutex<HashMap<ChunkHash, Pending>>,
    /// Single-flight map, present only while a fetch is running.
    inflight: Mutex<HashMap<ChunkHash, watch::Receiver<Option<SharedResult>>>>,
    store_index: Arc<RwLock<StoreIndex>>,
    /// Landed chunks awaiting the next [`CHUNK_FLUSH_INTERVAL`] flush.
    /// Keyed per manifest because a SHA only has meaningful file paths
    /// inside the manifest that referenced it.
    pending_chunks: Mutex<HashMap<(DepotId, ManifestId), PendingChunkBatch>>,
    /// Set while a flush task is scheduled, so only one timer runs.
    flush_scheduled: AtomicBool,
}

/// `VfsError` isn't `Clone`, so followers get the stringified error.
type SharedResult = Result<Bytes, String>;

struct Pending {
    size_compressed: u64,
    /// Keeps the manifest alive for [`ChunkService::record_chunk_landed`].
    snapshot: Arc<Snapshot>,
    waiters: Vec<oneshot::Sender<bool>>,
}

struct Job {
    sha: ChunkHash,
    snapshot: Arc<Snapshot>,
}

struct PendingChunkBatch {
    snapshot: Arc<Snapshot>,
    shas: HashSet<ChunkHash>,
}

#[derive(Default, Clone, Debug, Serialize, ToSchema)]
#[schema(example = json!({
    "chunks_total": 14586,
    "chunks_completed": 1003,
    "chunks_failed": 0,
    "bytes_total": 11283371566u64,
    "bytes_completed": 993842156u64,
    "last_error": null
}))]
pub struct DownloadStats {
    pub chunks_total: u64,
    pub chunks_completed: u64,
    pub chunks_failed: u64,
    pub bytes_total: u64,
    pub bytes_completed: u64,
    /// Most recent failure message, if any.
    pub last_error: Option<String>,
}

#[derive(Default, Debug, Serialize, ToSchema)]
#[schema(example = json!({"enqueued_chunks": 14586, "enqueued_bytes": 11283371566u64, "already_present_chunks": 0}))]
pub struct EnqueueSummary {
    pub enqueued_chunks: u64,
    pub enqueued_bytes: u64,
    pub already_present_chunks: u64,
}

/// Multiplexed event on the broadcast channel; each variant maps to a
/// distinct SSE `event:` name.
#[derive(Clone, Debug, Serialize)]
#[serde(untagged)]
pub enum DownloadEvent {
    Stats(DownloadStats),
    Chunks(ChunkUpdate),
}

/// Coalesced "chunks landed" notice for a single manifest.
/// `chunks_present` is the authoritative new count, not a delta, so
/// missed events self-heal on the next one.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct ChunkUpdate {
    pub depot_id: DepotId,
    pub manifest_id: ManifestId,
    pub files: Vec<FileChunksUpdate>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct FileChunksUpdate {
    pub path: String,
    pub chunks_present: u32,
}

impl ChunkService {
    pub fn spawn(store_index: Arc<RwLock<StoreIndex>>) -> Arc<Self> {
        let (interactive, mut int_rx) = mpsc::unbounded_channel::<Job>();
        let (bulk, mut bulk_rx) = mpsc::unbounded_channel::<Job>();
        let (events, _) = broadcast::channel(64);
        let service = Arc::new(Self {
            interactive,
            bulk,
            stats: Mutex::new(DownloadStats::default()),
            events,
            pending: Mutex::new(HashMap::new()),
            inflight: Mutex::new(HashMap::new()),
            store_index,
            pending_chunks: Mutex::new(HashMap::new()),
            flush_scheduled: AtomicBool::new(false),
        });

        let worker = Arc::downgrade(&service);
        let parallelism = parallelism();
        let throttle = throttle_ms();
        tracing::info!(parallelism, throttle_ms = throttle, "chunk service started");
        tokio::spawn(async move {
            let sem = Arc::new(Semaphore::new(parallelism));
            loop {
                // Biased: interactive jobs overtake queued bulk work.
                let job = tokio::select! {
                    biased;
                    j = int_rx.recv() => j,
                    j = bulk_rx.recv() => j,
                };
                let Some(job) = job else { break };
                let permit = sem
                    .clone()
                    .acquire_owned()
                    .await
                    .expect("semaphore not closed");
                let Some(worker) = worker.upgrade() else {
                    break;
                };
                tokio::spawn(async move {
                    let _permit = permit;
                    // Settled in the meantime or cancelled: skip.
                    if !worker
                        .pending
                        .lock()
                        .expect("pending poisoned")
                        .contains_key(&job.sha)
                    {
                        return;
                    }
                    if throttle > 0 {
                        tokio::time::sleep(Duration::from_millis(throttle)).await;
                    }
                    let res = job.snapshot.chunks().ensure(job.sha).await;
                    // Fallback for disk-cache hits, which never reach the
                    // fetch_via layer that normally settles.
                    let err = res.as_ref().err().map(|e| e.to_string());
                    worker.settle(job.sha, res.is_ok(), err);
                });
            }
        });

        service
    }

    /// Enqueue as background bulk work; chunks already on disk or queued
    /// are skipped.
    pub fn enqueue(
        &self,
        snapshot: Arc<Snapshot>,
        chunks: impl IntoIterator<Item = (ChunkHash, u64)>,
    ) -> EnqueueSummary {
        self.enqueue_internal(snapshot, chunks, Priority::Bulk, false)
            .0
    }

    /// Like [`enqueue`](Self::enqueue), but interactive: jumps ahead of
    /// bulk work and waits until every requested chunk landed or failed.
    pub async fn enqueue_and_wait(
        &self,
        snapshot: Arc<Snapshot>,
        chunks: impl IntoIterator<Item = (ChunkHash, u64)>,
    ) -> EnqueueSummary {
        let (summary, waiters) =
            self.enqueue_internal(snapshot, chunks, Priority::Interactive, true);
        for rx in waiters {
            // Err means cancelled; the caller discovers missing chunks
            // via the on-disk state.
            let _ = rx.await;
        }
        summary
    }

    fn enqueue_internal(
        &self,
        snapshot: Arc<Snapshot>,
        chunks: impl IntoIterator<Item = (ChunkHash, u64)>,
        priority: Priority,
        register_waiters: bool,
    ) -> (EnqueueSummary, Vec<oneshot::Receiver<bool>>) {
        let lane = match priority {
            Priority::Interactive => &self.interactive,
            Priority::Bulk => &self.bulk,
        };
        let mut summary = EnqueueSummary::default();
        let mut waiters = Vec::new();
        {
            let index = self.store_index.read().expect("store_index poisoned");
            let mut pending = self.pending.lock().expect("pending poisoned");
            for (sha, size) in chunks {
                if index.has_chunk(&sha) {
                    summary.already_present_chunks += 1;
                    continue;
                }
                match pending.entry(sha) {
                    std::collections::hash_map::Entry::Occupied(mut e) => {
                        // Already queued and counted. Re-submitting on the
                        // interactive lane overtakes the bulk queue; the
                        // stale bulk job sees the chunk settled and skips.
                        if register_waiters {
                            let (tx, rx) = oneshot::channel();
                            e.get_mut().waiters.push(tx);
                            waiters.push(rx);
                        }
                        if matches!(priority, Priority::Interactive) {
                            let _ = lane.send(Job {
                                sha,
                                snapshot: snapshot.clone(),
                            });
                        }
                    }
                    std::collections::hash_map::Entry::Vacant(e) => {
                        let mut entry = Pending {
                            size_compressed: size,
                            snapshot: snapshot.clone(),
                            waiters: Vec::new(),
                        };
                        if register_waiters {
                            let (tx, rx) = oneshot::channel();
                            entry.waiters.push(tx);
                            waiters.push(rx);
                        }
                        e.insert(entry);
                        if lane
                            .send(Job {
                                sha,
                                snapshot: snapshot.clone(),
                            })
                            .is_err()
                        {
                            // Worker has shut down; abort the rest.
                            break;
                        }
                        summary.enqueued_chunks += 1;
                        summary.enqueued_bytes += size;
                    }
                }
            }
        }
        if summary.enqueued_chunks > 0 {
            let snapshot_stats = {
                let mut s = self.stats.lock().expect("stats poisoned");
                s.chunks_total += summary.enqueued_chunks;
                s.bytes_total += summary.enqueued_bytes;
                s.clone()
            };
            let _ = self.events.send(DownloadEvent::Stats(snapshot_stats));
        }
        (summary, waiters)
    }

    /// Single-flight + accounting wrapper around an actual CDN fetch;
    /// the only place where chunk completions are counted.
    /// Cancellation-safe: if the leader is dropped mid-fetch, followers
    /// retry and one becomes the new leader.
    pub async fn fetch_via<F>(self: &Arc<Self>, sha: ChunkHash, fetch: F) -> Result<Bytes, VfsError>
    where
        F: Future<Output = Result<Bytes, VfsError>>,
    {
        enum Role {
            Leader(watch::Sender<Option<SharedResult>>),
            Follower(watch::Receiver<Option<SharedResult>>),
        }
        let mut fetch = Some(fetch);
        loop {
            // Decide the role under the lock, fetch outside it.
            let role = {
                let mut inflight = self.inflight.lock().expect("inflight poisoned");
                match inflight.entry(sha) {
                    std::collections::hash_map::Entry::Occupied(e) => {
                        Role::Follower(e.get().clone())
                    }
                    std::collections::hash_map::Entry::Vacant(e) => {
                        let (tx, rx) = watch::channel(None);
                        e.insert(rx);
                        Role::Leader(tx)
                    }
                }
            };
            let mut rx = match role {
                Role::Leader(tx) => {
                    return self
                        .lead_fetch(sha, fetch.take().expect("leader runs once"), tx)
                        .await;
                }
                Role::Follower(rx) => rx,
            };
            loop {
                if let Some(shared) = rx.borrow_and_update().clone() {
                    return shared.map_err(|msg| VfsError::Other(msg.into()));
                }
                if rx.changed().await.is_err() {
                    // Leader dropped without a result: clean up and retry.
                    let mut inflight = self.inflight.lock().expect("inflight poisoned");
                    if let Some(cur) = inflight.get(&sha)
                        && cur.same_channel(&rx)
                    {
                        inflight.remove(&sha);
                    }
                    break;
                }
            }
        }
    }

    async fn lead_fetch<F>(
        self: &Arc<Self>,
        sha: ChunkHash,
        fetch: F,
        tx: watch::Sender<Option<SharedResult>>,
    ) -> Result<Bytes, VfsError>
    where
        F: Future<Output = Result<Bytes, VfsError>>,
    {
        // Removes the inflight entry on drop, so followers don't hang
        // on a dead leader.
        struct InflightGuard<'a> {
            service: &'a ChunkService,
            sha: ChunkHash,
        }
        impl Drop for InflightGuard<'_> {
            fn drop(&mut self) {
                self.service
                    .inflight
                    .lock()
                    .expect("inflight poisoned")
                    .remove(&self.sha);
            }
        }
        let guard = InflightGuard { service: self, sha };

        // Queued chunks were counted at enqueue, implicit reads here.
        let queued = self
            .pending
            .lock()
            .expect("pending poisoned")
            .contains_key(&sha);
        if !queued {
            let snapshot = {
                let mut s = self.stats.lock().expect("stats poisoned");
                s.chunks_total += 1;
                s.clone()
            };
            let _ = self.events.send(DownloadEvent::Stats(snapshot));
        }

        let res = fetch.await;

        match &res {
            Ok(bytes) => {
                if queued {
                    self.settle(sha, true, None);
                } else {
                    // Implicit read: size unknown upfront, count the
                    // delivered bytes into total and completed alike.
                    self.store_index
                        .write()
                        .expect("store_index poisoned")
                        .mark_chunk_present(sha);
                    let snapshot = {
                        let mut s = self.stats.lock().expect("stats poisoned");
                        s.chunks_completed += 1;
                        s.bytes_total += bytes.len() as u64;
                        s.bytes_completed += bytes.len() as u64;
                        s.clone()
                    };
                    let _ = self.events.send(DownloadEvent::Stats(snapshot));
                }
            }
            Err(err) => {
                tracing::warn!(%sha, %err, "chunk fetch failed");
                if queued {
                    self.settle(sha, false, Some(err.to_string()));
                } else {
                    let snapshot = {
                        let mut s = self.stats.lock().expect("stats poisoned");
                        s.chunks_failed += 1;
                        s.last_error = Some(err.to_string());
                        s.clone()
                    };
                    let _ = self.events.send(DownloadEvent::Stats(snapshot));
                }
            }
        }

        let shared: SharedResult = res.as_ref().map(Bytes::clone).map_err(|e| e.to_string());
        // Remove the inflight entry before publishing so a late-comer
        // becomes a fresh leader and hits the disk cache.
        drop(guard);
        let _ = tx.send(Some(shared));
        res
    }

    /// Settle a queued chunk exactly once: update stats, mark disk
    /// presence, notify waiters. No-op if already settled or cancelled.
    fn settle(self: &Arc<Self>, sha: ChunkHash, ok: bool, err: Option<String>) {
        let Some(entry) = self.pending.lock().expect("pending poisoned").remove(&sha) else {
            return;
        };
        if ok {
            // Inline (not spawned) so a waiter never resolves before the
            // index reflects the chunk.
            self.store_index
                .write()
                .expect("store_index poisoned")
                .mark_chunk_present(sha);
        }
        let snapshot = {
            let mut s = self.stats.lock().expect("stats poisoned");
            if ok {
                s.chunks_completed += 1;
                s.bytes_completed += entry.size_compressed;
            } else {
                s.chunks_failed += 1;
                s.last_error = err;
            }
            s.clone()
        };
        let _ = self.events.send(DownloadEvent::Stats(snapshot));
        for w in entry.waiters {
            let _ = w.send(ok);
        }
        if ok {
            self.record_chunk_landed(entry.snapshot, sha);
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<DownloadEvent> {
        self.events.subscribe()
    }

    pub fn current(&self) -> DownloadStats {
        self.stats.lock().expect("stats poisoned").clone()
    }

    /// Drop all queued work and reset counters to zero. Chunks already
    /// in-flight still land on disk but won't tick the zeroed stats.
    pub fn cancel(&self) {
        // Dropping the waiter senders signals every awaiter with Err.
        // Queued jobs see their pending entry gone and skip.
        self.pending.lock().expect("pending poisoned").clear();
        self.pending_chunks
            .lock()
            .expect("pending_chunks poisoned")
            .clear();
        let snapshot = {
            let mut s = self.stats.lock().expect("stats poisoned");
            *s = DownloadStats::default();
            s.clone()
        };
        let _ = self.events.send(DownloadEvent::Stats(snapshot));
        tracing::info!("download queue cancelled");
    }

    /// Batch a landed chunk for the next per-file SSE flush.
    fn record_chunk_landed(self: &Arc<Self>, snapshot: Arc<Snapshot>, sha: ChunkHash) {
        {
            let m = snapshot.manifest();
            let key = (DepotId(m.depot_id), ManifestId(m.manifest_id));
            let mut pending = self.pending_chunks.lock().expect("pending poisoned");
            pending
                .entry(key)
                .or_insert_with(|| PendingChunkBatch {
                    snapshot: snapshot.clone(),
                    shas: HashSet::new(),
                })
                .shas
                .insert(sha);
        }
        if !self.flush_scheduled.swap(true, Ordering::AcqRel) {
            let this = Arc::clone(self);
            tokio::spawn(async move {
                tokio::time::sleep(CHUNK_FLUSH_INTERVAL).await;
                this.flush_scheduled.store(false, Ordering::Release);
                this.flush_pending_chunks();
            });
        }
    }

    /// Drain pending chunk landings and broadcast one [`ChunkUpdate`] per
    /// affected manifest. Iterates `manifest.files`, but at most once per
    /// [`CHUNK_FLUSH_INTERVAL`].
    fn flush_pending_chunks(&self) {
        let drained = {
            let mut pending = self.pending_chunks.lock().expect("pending poisoned");
            std::mem::take(&mut *pending)
        };
        if drained.is_empty() {
            return;
        }
        let index = self.store_index.read().expect("store_index poisoned");
        for ((depot_id, manifest_id), batch) in drained {
            let manifest = batch.snapshot.manifest();
            let mut files = Vec::new();
            for f in &manifest.files {
                let mut touched = false;
                let mut present = 0u32;
                for c in &f.chunks {
                    if batch.shas.contains(&c.sha) {
                        touched = true;
                    }
                    if index.has_chunk(&c.sha) {
                        present += 1;
                    }
                }
                if touched {
                    files.push(FileChunksUpdate {
                        path: f.path.clone(),
                        chunks_present: present,
                    });
                }
            }
            if !files.is_empty() {
                let _ = self.events.send(DownloadEvent::Chunks(ChunkUpdate {
                    depot_id,
                    manifest_id,
                    files,
                }));
            }
        }
    }
}
