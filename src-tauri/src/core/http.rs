//! The direct-HTTP transfer used by the embedded backend. [`download`] probes the
//! server, then either splits the file across parallel connections (see
//! [`super::segmented`]) or streams it over one. Both paths are resumable against
//! the existing `.part` file.

use std::time::Duration;

use futures_util::StreamExt;
use reqwest::header::{CONTENT_RANGE, RANGE};
use reqwest::Client;
use tokio::io::AsyncWriteExt;

use super::backend::{Control, Outcome, ProgressFn, Signal};
use super::fsattr;
use super::segmented;

/// Download `url` into `part`, then rename to `dest` on success. Uses up to
/// `connections` parallel streams when the server supports ranges and the file is
/// large enough; otherwise a single resumable stream. `min_split` is the smallest
/// piece worth its own connection — a file below it stays single-stream, and it
/// caps how finely a larger one is split. `meta_path` stores the resume plan.
#[allow(clippy::too_many_arguments)]
pub async fn download(
    client: &Client,
    url: &str,
    part: &str,
    dest: &str,
    meta_path: &str,
    connections: usize,
    min_split: u64,
    hide_part: bool,
    control: &Control,
    progress: &ProgressFn,
) -> Outcome {
    let plan = probe(client, url).await;

    // A saved multi-connection plan that still matches the server's size resumes
    // as segmented, always: the `.part` is pre-sized for positioned writes, and a
    // single-stream pass over it would treat the padding as real data.
    if let (true, Some(total)) = (plan.ranges, plan.total) {
        if segmented::plan_matches(meta_path, part, total).await {
            return segmented::download(
                client,
                url,
                part,
                dest,
                meta_path,
                total,
                connections,
                min_split,
                hide_part,
                control,
                progress,
            )
            .await;
        }
    }

    // No usable plan. Clear any stale segmented artifacts so the single-stream
    // fallback can't mistake a pre-sized `.part` for a finished download.
    if tokio::fs::metadata(meta_path).await.is_ok() {
        let _ = tokio::fs::remove_file(meta_path).await;
        let _ = tokio::fs::remove_file(part).await;
    }

    // Fresh multi-connection download when the server allows it, it's big enough,
    // and there's no single-stream partial already on disk to preserve.
    if connections > 1 {
        if let (true, Some(total)) = (plan.ranges, plan.total) {
            if total >= min_split && existing_len(part).await == 0 {
                return segmented::download(
                    client,
                    url,
                    part,
                    dest,
                    meta_path,
                    total,
                    connections,
                    min_split,
                    hide_part,
                    control,
                    progress,
                )
                .await;
            }
        }
    }

    single_stream(client, url, part, dest, hide_part, control, progress).await
}

/// What a server will let us do with a URL, learned from a tiny ranged request.
struct Plan {
    /// Total size in bytes, when the server reports it.
    total: Option<u64>,
    /// Whether the server honored a `Range` request (a `206` reply).
    ranges: bool,
}

/// Ask for a single byte with a `Range` header. A `206` back means ranges work
/// and the `Content-Range` carries the full size; the body is dropped unread.
async fn probe(client: &Client, url: &str) -> Plan {
    let resp = match client.get(url).header(RANGE, "bytes=0-0").send().await {
        Ok(r) => r,
        // Let the real transfer surface the error; assume single-stream for now.
        Err(_) => {
            return Plan {
                total: None,
                ranges: false,
            }
        }
    };
    if resp.status().as_u16() == 206 {
        Plan {
            total: content_range_total(&resp),
            ranges: true,
        }
    } else if resp.status().is_success() {
        Plan {
            total: resp.content_length(),
            ranges: false,
        }
    } else {
        Plan {
            total: None,
            ranges: false,
        }
    }
}

/// Bytes already sitting in `part`, or 0 if it isn't there.
async fn existing_len(part: &str) -> u64 {
    tokio::fs::metadata(part)
        .await
        .map(|m| m.len())
        .unwrap_or(0)
}

/// Stream `url` into `part` over one connection, resuming from whatever is already
/// there, then rename to `dest`.
async fn single_stream(
    client: &Client,
    url: &str,
    part: &str,
    dest: &str,
    hide_part: bool,
    control: &Control,
    progress: &ProgressFn,
) -> Outcome {
    // Resume offset = bytes already on disk.
    let offset = tokio::fs::metadata(part)
        .await
        .map(|m| m.len())
        .unwrap_or(0);

    let mut req = client.get(url);
    if offset > 0 {
        req = req.header(RANGE, format!("bytes={offset}-"));
    }

    let resp = match req.send().await {
        Ok(r) => r,
        Err(e) => return Outcome::Failed(friendly(&e)),
    };

    let code = resp.status();
    // Already have the whole file: the server says our range is unsatisfiable.
    if code.as_u16() == 416 && offset > 0 {
        return finalize(part, dest).await;
    }
    if !code.is_success() {
        return Outcome::Failed(format!("server returned {code}"));
    }

    // 206 = resuming; anything else means we start from the top (the server
    // ignored our Range), so the partial file is discarded.
    let resuming = code.as_u16() == 206;
    let total = if resuming {
        content_range_total(&resp)
    } else {
        resp.content_length()
    };
    let start = if resuming { offset } else { 0 };

    let mut file = match open_part(part, resuming && start > 0).await {
        Ok(f) => {
            if hide_part {
                fsattr::set_hidden(part, true);
            }
            f
        }
        Err(e) => return Outcome::Failed(e),
    };

    let mut received = start;
    progress(received, total);

    // How long to wait for a chunk before waking to re-check pause/cancel, and
    // how many such stalls in a row mean the connection is dead.
    const POLL: Duration = Duration::from_secs(2);
    const STALL_LIMIT: u32 = 30; // ~60s with no data → give up

    let mut stream = resp.bytes_stream();
    let mut stalls = 0u32;
    loop {
        // Checked every POLL even mid-stall, so a cancel/pause (e.g. from a
        // remove) always takes effect and the file handle is released promptly.
        match control.signal() {
            Signal::Pause => {
                let _ = file.flush().await;
                return Outcome::Paused;
            }
            Signal::Cancel => {
                let _ = file.flush().await;
                return Outcome::Canceled;
            }
            Signal::Run => {}
        }

        let chunk = match tokio::time::timeout(POLL, stream.next()).await {
            Ok(Some(chunk)) => chunk,
            Ok(None) => break, // stream finished
            Err(_) => {
                stalls += 1;
                if stalls >= STALL_LIMIT {
                    let _ = file.flush().await;
                    return Outcome::Failed("connection stalled".to_string());
                }
                continue;
            }
        };
        stalls = 0;

        let bytes = match chunk {
            Ok(b) => b,
            Err(e) => {
                let _ = file.flush().await;
                return Outcome::Failed(friendly(&e));
            }
        };
        if let Err(e) = file.write_all(&bytes).await {
            return Outcome::Failed(format!("write failed: {e}"));
        }
        received += bytes.len() as u64;
        progress(received, total);
    }

    if let Err(e) = file.flush().await {
        return Outcome::Failed(format!("write failed: {e}"));
    }
    drop(file);

    if let Some(t) = total {
        if received < t {
            return Outcome::Failed(format!("connection closed early ({received} of {t} bytes)"));
        }
    }

    finalize(part, dest).await
}

/// Move the finished `.part` file to its final name. Clears the hidden attribute
/// so a finished download is always visible, even if the `.part` was hidden while
/// downloading (a no-op when it wasn't).
pub(super) async fn finalize(part: &str, dest: &str) -> Outcome {
    match tokio::fs::rename(part, dest).await {
        Ok(()) => {
            fsattr::set_hidden(dest, false);
            Outcome::Completed
        }
        Err(e) => Outcome::Failed(format!("couldn't finalize file: {e}")),
    }
}

async fn open_part(path: &str, append: bool) -> Result<tokio::fs::File, String> {
    let mut opts = tokio::fs::OpenOptions::new();
    opts.write(true).create(true);
    if append {
        opts.append(true);
    } else {
        opts.truncate(true);
    }
    opts.open(path)
        .await
        .map_err(|e| format!("couldn't open {path}: {e}"))
}

/// Total size from a response's `Content-Range` header, if present.
fn content_range_total(resp: &reqwest::Response) -> Option<u64> {
    let raw = resp.headers().get(CONTENT_RANGE)?.to_str().ok()?;
    parse_content_range_total(raw)
}

/// Parse the total out of a `Content-Range: bytes start-end/total` value.
/// Returns `None` for an unknown ("*") total or a malformed header.
fn parse_content_range_total(raw: &str) -> Option<u64> {
    let total = raw.rsplit('/').next()?.trim();
    if total == "*" {
        None
    } else {
        total.parse().ok()
    }
}

pub(super) fn friendly(e: &reqwest::Error) -> String {
    if e.is_timeout() {
        "connection timed out".to_string()
    } else if e.is_connect() {
        "couldn't reach the server".to_string()
    } else {
        e.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::parse_content_range_total;

    #[test]
    fn content_range_total_parses_normal_header() {
        assert_eq!(parse_content_range_total("bytes 200-1000/1001"), Some(1001));
        assert_eq!(parse_content_range_total("bytes 0-0/500"), Some(500));
    }

    #[test]
    fn content_range_total_is_none_when_unknown_or_malformed() {
        assert_eq!(parse_content_range_total("bytes 200-1000/*"), None);
        assert_eq!(parse_content_range_total("garbage"), None);
    }
}
