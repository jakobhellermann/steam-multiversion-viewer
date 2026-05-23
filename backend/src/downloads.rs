// TODO(ai-review): review for style and correctness
//! Background chunk downloader.
//!
//! Routes enqueue (snapshot, sha) pairs; a long-lived worker drains them
//! with bounded parallelism and broadcasts progress so the UI can render a
//! live status drawer. Chunks already present on disk or already queued
//! in this session are skipped, so clicking "download" twice is cheap.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use serde::Serialize;
use steam_depot_vfs::ChunkHash;
use steam_depot_vfs::chunk_store::ChunkStore;
use tokio::sync::{Semaphore, broadcast, mpsc, oneshot};
use utoipa::ToSchema;

use crate::state::Snapshot;
use crate::steam::{DepotId, ManifestId};
use crate::store_index::StoreIndex;

/// Coalesce window for the per-file `chunks` SSE events. Multiple chunk
/// landings inside this window collapse into one event per affected
/// (manifest, file) pair. Trades a tiny UI latency for far fewer events
/// on the wire during a busy parallel download (16-32 chunks/s easy).
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

/// Optional per-chunk delay knob, in milliseconds. Useful for stretching
/// out a download so progress UI can be observed end-to-end. Debug-only;
/// release builds have no point paying the syscall.
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

pub struct DownloadManager {
    submit: mpsc::UnboundedSender<Job>,
    stats: Mutex<DownloadStats>,
    events: broadcast::Sender<DownloadEvent>,
    seen: Mutex<HashSet<ChunkHash>>,
    store_index: Arc<RwLock<StoreIndex>>,
    /// Bumped on cancel. Jobs carry the epoch they were enqueued under;
    /// any stats update from a stale-epoch job is suppressed.
    epoch: AtomicU64,
    /// One-shot completion notifications, keyed by chunk SHA. Callers of
    /// [`DownloadManager::enqueue_and_wait`] register a sender per chunk
    /// they need to wait on; the worker fires them after the chunk lands
    /// (or fails) so the awaiter can proceed.
    waiters: Mutex<HashMap<ChunkHash, Vec<oneshot::Sender<bool>>>>,
    /// Landed chunks awaiting the next [`CHUNK_FLUSH_INTERVAL`] flush, so
    /// per-file `chunks` events get batched instead of one-per-landing.
    /// Keyed by (depot_id, manifest_id) because a single chunk SHA only
    /// has meaningful file-paths inside the manifest that referenced it.
    /// The `Arc<Snapshot>` keeps the manifest alive for the flush task's
    /// scan over `manifest.files`.
    pending_chunks: Mutex<HashMap<(DepotId, ManifestId), PendingChunkBatch>>,
    /// Set while a flush task is scheduled; ensures only one timer runs
    /// at a time. Reset by the flush task before it drains.
    flush_scheduled: AtomicBool,
}

struct PendingChunkBatch {
    snapshot: Arc<Snapshot>,
    shas: HashSet<ChunkHash>,
}

struct Job {
    sha: ChunkHash,
    size_compressed: u64,
    snapshot: Arc<Snapshot>,
    epoch: u64,
}

#[derive(Default, Clone, Debug, Serialize, ToSchema)]
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
pub struct EnqueueSummary {
    pub enqueued_chunks: u64,
    pub enqueued_bytes: u64,
    pub already_present_chunks: u64,
}

/// Multiplexed event on the [`DownloadManager`] broadcast channel. The
/// SSE handler maps each variant to a distinct SSE `event:` name so the
/// frontend can register typed listeners.
#[derive(Clone, Debug, Serialize)]
#[serde(untagged)]
pub enum DownloadEvent {
    Stats(DownloadStats),
    Chunks(ChunkUpdate),
}

/// Coalesced "chunks landed" notice for a single manifest. The frontend
/// uses it to patch its `manifest-files` query cache in place without a
/// refetch — `path` keys into the visible rows, `chunks_present` is the
/// authoritative new count (not a delta, so missed events self-heal on
/// the next one).
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

impl DownloadManager {
    pub fn spawn(store_index: Arc<RwLock<StoreIndex>>) -> Arc<Self> {
        let (submit, mut rx) = mpsc::unbounded_channel::<Job>();
        let (events, _) = broadcast::channel(64);
        let manager = Arc::new(Self {
            submit,
            stats: Mutex::new(DownloadStats::default()),
            events,
            seen: Mutex::new(HashSet::new()),
            store_index,
            epoch: AtomicU64::new(0),
            waiters: Mutex::new(HashMap::new()),
            pending_chunks: Mutex::new(HashMap::new()),
            flush_scheduled: AtomicBool::new(false),
        });

        let worker = manager.clone();
        let parallelism = parallelism();
        let throttle = throttle_ms();
        tracing::info!(
            parallelism,
            throttle_ms = throttle,
            "download worker started"
        );
        tokio::spawn(async move {
            let sem = Arc::new(Semaphore::new(parallelism));
            while let Some(job) = rx.recv().await {
                let permit = sem
                    .clone()
                    .acquire_owned()
                    .await
                    .expect("semaphore not closed");
                let worker = worker.clone();
                tokio::spawn(async move {
                    let _permit = permit;
                    // Drop queued jobs whose enqueue epoch has been
                    // superseded by a cancel — no point burning CDN
                    // bandwidth on a download the user already aborted.
                    if job.epoch < worker.epoch.load(Ordering::Acquire) {
                        return;
                    }
                    if throttle > 0 {
                        tokio::time::sleep(Duration::from_millis(throttle)).await;
                    }
                    let res = job.snapshot.chunks().ensure(job.sha).await;
                    let ok = res.is_ok();
                    worker.complete(job.sha, job.size_compressed, res, job.epoch);
                    if ok {
                        Arc::clone(&worker).record_chunk_landed(job.snapshot, job.sha);
                    }
                });
            }
        });

        manager
    }

    /// Filters out chunks that are already on disk or already queued in this
    /// session, enqueues the rest. The summary tells the caller what
    /// actually got submitted vs. what was a no-op.
    pub async fn enqueue(
        &self,
        snapshot: Arc<Snapshot>,
        chunks: impl IntoIterator<Item = (ChunkHash, u64)>,
    ) -> EnqueueSummary {
        self.enqueue_internal(snapshot, chunks, false).await.0
    }

    /// Like [`enqueue`], but also waits until every requested chunk has
    /// either landed on disk (and been recorded in the chunk-presence
    /// index) or failed. Chunks already on disk return immediately;
    /// chunks already in flight from a prior enqueue still get a waiter
    /// attached so the caller sees the same completion semantics.
    pub async fn enqueue_and_wait(
        &self,
        snapshot: Arc<Snapshot>,
        chunks: impl IntoIterator<Item = (ChunkHash, u64)>,
    ) -> EnqueueSummary {
        let (summary, waiters) = self.enqueue_internal(snapshot, chunks, true).await;
        for rx in waiters {
            // Err just means the sender was dropped (cancelled or worker
            // crashed). Either way the caller will discover the missing
            // chunks via the on-disk state.
            let _ = rx.await;
        }
        summary
    }

    async fn enqueue_internal(
        &self,
        snapshot: Arc<Snapshot>,
        chunks: impl IntoIterator<Item = (ChunkHash, u64)>,
        register_waiters: bool,
    ) -> (EnqueueSummary, Vec<oneshot::Receiver<bool>>) {
        let mut summary = EnqueueSummary::default();
        let mut waiters = Vec::new();
        let epoch = self.epoch.load(Ordering::Acquire);
        {
            let index = self.store_index.read().expect("store_index poisoned");
            let mut seen = self.seen.lock().expect("seen poisoned");
            let mut waitmap_guard = if register_waiters {
                Some(self.waiters.lock().expect("waiters poisoned"))
            } else {
                None
            };
            for (sha, size) in chunks {
                if index.has_chunk(&sha) {
                    summary.already_present_chunks += 1;
                    continue;
                }
                if let Some(ref mut waitmap) = waitmap_guard {
                    let (tx, rx) = oneshot::channel();
                    waitmap.entry(sha).or_default().push(tx);
                    waiters.push(rx);
                }
                if !seen.insert(sha) {
                    // Another caller already enqueued this sha. Our
                    // waiter (if any) will fire alongside theirs.
                    continue;
                }
                if self
                    .submit
                    .send(Job {
                        sha,
                        size_compressed: size,
                        snapshot: snapshot.clone(),
                        epoch,
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

    pub fn subscribe(&self) -> broadcast::Receiver<DownloadEvent> {
        self.events.subscribe()
    }

    pub fn current(&self) -> DownloadStats {
        self.stats.lock().expect("stats poisoned").clone()
    }

    /// Reset counters to zero and bump the epoch. Any chunks already
    /// in-flight will land on disk (and update the chunks-present index)
    /// but won't update the freshly-zeroed stats.
    pub fn cancel(&self) {
        self.epoch.fetch_add(1, Ordering::AcqRel);
        self.seen.lock().expect("seen poisoned").clear();
        // Dropping the senders signals every awaiter with Err — they
        // fall back to direct fetches via FsCacheStore rather than
        // hanging forever.
        self.waiters.lock().expect("waiters poisoned").clear();
        // Pending coalesced chunk updates from this run are no longer
        // interesting; let the next flush start empty.
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

    fn complete(
        &self,
        sha: ChunkHash,
        size: u64,
        res: Result<(), steam_depot_vfs::VfsError>,
        job_epoch: u64,
    ) {
        // Stats update first — independent of any waiter or index work,
        // and we want the broadcast snapshot to fire even if the chunk
        // failed.
        let stale = job_epoch < self.epoch.load(Ordering::Acquire);
        let ok = res.is_ok();
        if !stale {
            let snapshot = {
                let mut s = self.stats.lock().expect("stats poisoned");
                match res {
                    Ok(()) => {
                        s.chunks_completed += 1;
                        s.bytes_completed += size;
                    }
                    Err(err) => {
                        s.chunks_failed += 1;
                        s.last_error = Some(err.to_string());
                        tracing::warn!(%sha, %err, "chunk fetch failed");
                    }
                }
                s.clone()
            };
            let _ = self.events.send(DownloadEvent::Stats(snapshot));
        } else if let Err(err) = &res {
            tracing::warn!(%sha, %err, "chunk fetch failed (cancelled)");
        }

        if ok {
            // Sync mutex acquisition is fine here — `mark_chunk_present`
            // is a single HashSet::insert. Doing it inline (vs. spawning
            // an async task) avoids races where a waiter resolves before
            // the index reflects the chunk.
            self.store_index
                .write()
                .expect("store_index poisoned")
                .mark_chunk_present(sha);
        }
        if let Some(senders) = self.waiters.lock().expect("waiters poisoned").remove(&sha) {
            for s in senders {
                let _ = s.send(ok);
            }
        }
    }

    /// Record that a chunk just landed (and is now reflected in
    /// `store_index`). Adds the SHA to the pending batch for its manifest
    /// and, if no flush is already queued, spawns a deferred drain task.
    fn record_chunk_landed(self: Arc<Self>, snapshot: Arc<Snapshot>, sha: ChunkHash) {
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
            let this = self;
            tokio::spawn(async move {
                tokio::time::sleep(CHUNK_FLUSH_INTERVAL).await;
                this.flush_scheduled.store(false, Ordering::Release);
                this.flush_pending_chunks();
            });
        }
    }

    /// Drain pending chunk landings and broadcast one [`ChunkUpdate`] per
    /// affected manifest. NB: this iterates `manifest.files` once per
    /// pending manifest — O(N_files × avg_chunks_per_file) work, but
    /// only every [`CHUNK_FLUSH_INTERVAL`] (~10 Hz max) instead of per
    /// chunk landing.
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
