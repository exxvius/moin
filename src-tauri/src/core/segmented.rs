//! Multi-connection HTTP transfer: split the file into byte ranges and pull them
//! in parallel. The orchestration in [`super::http`] only routes here when the
//! server advertises range support and the file is worth splitting; anything else
//! stays on the single-stream path.
//!
//! Progress survives pause and restart through a small JSON sidecar (`.part.meta`)
//! that records where each segment left off. The `.part` file is pre-sized up
//! front so every worker can write straight into its own region with a positioned
//! handle — no shared cursor, no locking on the hot path.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::StreamExt;
use reqwest::header::{HeaderMap, RANGE};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncSeekExt, AsyncWriteExt};

use super::backend::{Control, Outcome, ProgressFn, Signal};
use super::fsattr;
use super::http::{finalize, friendly, stall_limit};

/// How long a worker waits on the socket before waking to re-check pause/cancel.
const POLL: Duration = Duration::from_secs(2);

/// One contiguous slice of the file, and how far it's been filled.
#[derive(Clone, Serialize, Deserialize)]
struct Seg {
    /// First byte of the slice (fixed for the life of the download).
    start: u64,
    /// Next absolute byte to write. Complete once it passes `end`.
    pos: u64,
    /// Last byte of the slice, inclusive.
    end: u64,
}

impl Seg {
    fn done(&self) -> bool {
        self.pos > self.end
    }

    /// Bytes still owed on this slice.
    fn remaining(&self) -> u64 {
        if self.done() {
            0
        } else {
            self.end - self.pos + 1
        }
    }
}

#[derive(Serialize, Deserialize)]
struct Meta {
    total: u64,
    segs: Vec<Seg>,
}

/// Whether a saved plan exists that still fits the server's current size — i.e.
/// this download can be resumed segment-by-segment rather than restarted.
pub async fn plan_matches(meta_path: &str, part: &str, total: u64) -> bool {
    let Some(meta) = load_meta(meta_path).await else {
        return false;
    };
    if meta.total != total {
        return false;
    }
    // The `.part` must still be the pre-sized file the segments write into.
    tokio::fs::metadata(part)
        .await
        .map(|m| m.len() == total)
        .unwrap_or(false)
}

/// Download `total` bytes of `url` into `part` across up to `connections` ranges,
/// each no smaller than `min_segment`, then rename to `dest`. Resumes from
/// `meta_path` when it holds a matching plan.
#[allow(clippy::too_many_arguments)]
pub async fn download(
    client: &Client,
    url: &str,
    headers: &HeaderMap,
    part: &str,
    dest: &str,
    meta_path: &str,
    total: u64,
    connections: usize,
    min_segment: u64,
    hide_part: bool,
    stall_timeout: Duration,
    control: &Control,
    progress: &ProgressFn,
) -> Outcome {
    let stall_max = stall_limit(stall_timeout, POLL);
    // Resume a saved plan, or lay out a fresh one and pre-size the file.
    let segs = if plan_matches(meta_path, part, total).await {
        match load_meta(meta_path).await {
            Some(meta) => meta.segs,
            None => return Outcome::Failed("couldn't read the resume plan".to_string()),
        }
    } else {
        let segs = split(total, connections, min_segment);
        if let Err(e) = preallocate(part, total).await {
            return Outcome::Failed(e);
        }
        segs
    };
    if hide_part {
        fsattr::set_hidden(part, true);
    }

    let received0: u64 = segs.iter().map(|s| s.pos - s.start).sum();
    let received = Arc::new(AtomicU64::new(received0));
    let shared = Arc::new(Mutex::new(segs.clone()));
    // Set by any worker that hits a hard error, so the rest stop pulling promptly.
    let aborted = Arc::new(AtomicBool::new(false));
    progress(received0, Some(total));

    let mut handles = Vec::new();
    for (idx, seg) in segs.iter().enumerate() {
        if seg.done() {
            continue;
        }
        let ctx = Worker {
            client: client.clone(),
            url: url.to_string(),
            headers: headers.clone(),
            part: part.to_string(),
            idx,
            seg: seg.clone(),
            total,
            stall_max,
            shared: shared.clone(),
            received: received.clone(),
            aborted: aborted.clone(),
            control: control.clone(),
            progress: progress.clone(),
        };
        handles.push(tokio::spawn(async move { ctx.run().await }));
    }

    let results = futures_util::future::join_all(handles).await;
    let final_segs = shared.lock().unwrap().clone();

    // Cancel wins over everything: the engine will wipe the .part and .meta.
    match control.signal() {
        Signal::Cancel => return Outcome::Canceled,
        Signal::Pause => {
            save_meta(meta_path, total, &final_segs).await;
            if hide_part {
                fsattr::set_hidden(meta_path, true);
            }
            return Outcome::Paused;
        }
        Signal::Run => {}
    }

    // Sort the workers' endings: a hard error wins over a stall (a real problem
    // beats "no data"), and either keeps the plan so a resume can pick up.
    let mut hard_error = None;
    let mut stalled = false;
    for r in results {
        match r {
            Ok(SegResult::Failed(e)) => {
                hard_error.get_or_insert(e);
            }
            Ok(SegResult::Stalled) => stalled = true,
            Ok(_) => {}
            Err(join) => {
                hard_error.get_or_insert(format!("a download worker crashed: {join}"));
            }
        }
    }
    if hard_error.is_some() || stalled {
        save_meta(meta_path, total, &final_segs).await;
        if hide_part {
            fsattr::set_hidden(meta_path, true);
        }
        return match hard_error {
            Some(err) => Outcome::Failed(err),
            None => Outcome::Stalled,
        };
    }

    // Every segment is full — drop the plan and promote the file.
    let _ = tokio::fs::remove_file(meta_path).await;
    finalize(part, dest).await
}

/// How a single segment's transfer ended.
enum SegResult {
    Done,
    /// Stopped on a pause/cancel signal or because a sibling aborted.
    Stopped,
    /// Went quiet past the stall window — no data, but no hard error either.
    Stalled,
    Failed(String),
}

struct Worker {
    client: Client,
    url: String,
    headers: HeaderMap,
    part: String,
    idx: usize,
    seg: Seg,
    total: u64,
    /// Consecutive `POLL` stalls this worker tolerates before giving up.
    stall_max: u32,
    shared: Arc<Mutex<Vec<Seg>>>,
    received: Arc<AtomicU64>,
    aborted: Arc<AtomicBool>,
    control: Control,
    progress: ProgressFn,
}

impl Worker {
    async fn run(mut self) -> SegResult {
        let mut file = match open_at(&self.part, self.seg.pos).await {
            Ok(f) => f,
            Err(e) => return self.fail(e),
        };

        let range = format!("bytes={}-{}", self.seg.pos, self.seg.end);
        let resp = match self
            .client
            .get(&self.url)
            .headers(self.headers.clone())
            .header(RANGE, range)
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => return self.fail(friendly(&e)),
        };
        if !resp.status().is_success() {
            return self.fail(format!("server returned {}", resp.status()));
        }

        let mut stream = resp.bytes_stream();
        let mut stalls = 0u32;
        loop {
            if self.aborted.load(Ordering::Relaxed) {
                let _ = file.flush().await;
                return SegResult::Stopped;
            }
            if self.control.signal() != Signal::Run {
                let _ = file.flush().await;
                return SegResult::Stopped;
            }

            let chunk = match tokio::time::timeout(POLL, stream.next()).await {
                Ok(Some(chunk)) => chunk,
                Ok(None) => break, // stream ended
                Err(_) => {
                    stalls += 1;
                    if stalls >= self.stall_max {
                        let _ = file.flush().await;
                        return self.stall();
                    }
                    continue;
                }
            };
            stalls = 0;

            let bytes = match chunk {
                Ok(b) => b,
                Err(e) => {
                    let _ = file.flush().await;
                    return self.fail(friendly(&e));
                }
            };
            // A server that ignores the range's end can overshoot; never write
            // past this segment into the next one's territory.
            let want = self.seg.remaining() as usize;
            let slice = if bytes.len() > want {
                &bytes[..want]
            } else {
                &bytes[..]
            };
            if let Err(e) = file.write_all(slice).await {
                return self.fail(format!("write failed: {e}"));
            }

            let n = slice.len() as u64;
            self.seg.pos += n;
            let total_recv = self.received.fetch_add(n, Ordering::Relaxed) + n;
            self.shared.lock().unwrap()[self.idx].pos = self.seg.pos;
            (self.progress)(total_recv, Some(self.total));

            if self.seg.done() {
                break;
            }
        }

        let _ = file.flush().await;
        if self.seg.done() {
            SegResult::Done
        } else {
            self.fail("connection closed early".to_string())
        }
    }

    /// Flag the abort so siblings wind down, then report the failure.
    fn fail(&self, msg: String) -> SegResult {
        self.aborted.store(true, Ordering::Relaxed);
        SegResult::Failed(msg)
    }

    /// Wind the siblings down and report a stall — the connection went quiet
    /// rather than erroring, so the whole download rests as stalled together.
    fn stall(&self) -> SegResult {
        self.aborted.store(true, Ordering::Relaxed);
        SegResult::Stalled
    }
}

/// Divide `[0, total)` into at most `connections` contiguous segments, each no
/// smaller than `min_segment`. `connections` is assumed to be at least 1.
fn split(total: u64, connections: usize, min_segment: u64) -> Vec<Seg> {
    let capacity = (total / min_segment.max(1)).max(1);
    let count = (connections as u64).clamp(1, capacity) as usize;
    let base = total / count as u64;

    let mut segs = Vec::with_capacity(count);
    let mut start = 0u64;
    for i in 0..count {
        let end = if i == count - 1 {
            total - 1
        } else {
            start + base - 1
        };
        segs.push(Seg {
            start,
            pos: start,
            end,
        });
        start = end + 1;
    }
    segs
}

/// Create (or resize) `part` to exactly `total` bytes so each worker can write
/// straight into its slice.
async fn preallocate(part: &str, total: u64) -> Result<(), String> {
    let file = tokio::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(part)
        .await
        .map_err(|e| format!("couldn't create {part}: {e}"))?;
    file.set_len(total)
        .await
        .map_err(|e| format!("couldn't size {part}: {e}"))?;
    Ok(())
}

/// Open `part` for writing and seek to `pos`, ready to fill one segment.
async fn open_at(part: &str, pos: u64) -> Result<tokio::fs::File, String> {
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .open(part)
        .await
        .map_err(|e| format!("couldn't open {part}: {e}"))?;
    file.seek(std::io::SeekFrom::Start(pos))
        .await
        .map_err(|e| format!("couldn't seek {part}: {e}"))?;
    Ok(file)
}

async fn load_meta(meta_path: &str) -> Option<Meta> {
    let raw = tokio::fs::read(meta_path).await.ok()?;
    serde_json::from_slice(&raw).ok()
}

/// Best-effort write of the resume plan. A lost meta only costs a restart, so a
/// failure here is swallowed rather than surfaced.
async fn save_meta(meta_path: &str, total: u64, segs: &[Seg]) {
    let meta = Meta {
        total,
        segs: segs.to_vec(),
    };
    if let Ok(bytes) = serde_json::to_vec(&meta) {
        let _ = tokio::fs::write(meta_path, bytes).await;
    }
}

#[cfg(test)]
mod tests {
    use super::split;

    /// A representative per-segment floor for the split tests.
    const MIN_SEGMENT: u64 = 256 * 1024;

    #[test]
    fn split_covers_the_whole_range_without_gaps() {
        let total = 10 * MIN_SEGMENT + 123;
        let segs = split(total, 4, MIN_SEGMENT);
        assert_eq!(segs.len(), 4);
        assert_eq!(segs[0].start, 0);
        assert_eq!(segs.last().unwrap().end, total - 1);
        for pair in segs.windows(2) {
            assert_eq!(pair[0].end + 1, pair[1].start);
        }
    }

    #[test]
    fn split_caps_segment_count_for_small_files() {
        // Only room for two MIN_SEGMENT slices, even if more are requested.
        let segs = split(2 * MIN_SEGMENT, 8, MIN_SEGMENT);
        assert_eq!(segs.len(), 2);
    }

    #[test]
    fn split_never_returns_zero_segments() {
        let segs = split(1, 8, MIN_SEGMENT);
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].start, 0);
        assert_eq!(segs[0].end, 0);
    }

    #[test]
    fn fresh_segments_start_empty() {
        let segs = split(4 * MIN_SEGMENT, 4, MIN_SEGMENT);
        assert!(segs.iter().all(|s| s.pos == s.start && !s.done()));
    }
}
