//! The direct-HTTP transfer used by the embedded backend. Single connection,
//! resumable via a `Range` request against the existing `.part` file. Multi-
//! segment parallelism can layer on top later without changing the caller.

use futures_util::StreamExt;
use reqwest::header::{CONTENT_RANGE, RANGE};
use reqwest::Client;
use tokio::io::AsyncWriteExt;

use super::backend::{Control, Outcome, ProgressFn, Signal};

/// Download `url` into `part`, then rename to `dest` on success. Resumes from
/// whatever is already in `part`.
pub async fn download(
    client: &Client,
    url: &str,
    part: &str,
    dest: &str,
    control: &Control,
    progress: &ProgressFn,
) -> Outcome {
    // Resume offset = bytes already on disk.
    let offset = tokio::fs::metadata(part).await.map(|m| m.len()).unwrap_or(0);

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
        Ok(f) => f,
        Err(e) => return Outcome::Failed(e),
    };

    let mut received = start;
    progress(received, total);

    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
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
            return Outcome::Failed(format!(
                "connection closed early ({received} of {t} bytes)"
            ));
        }
    }

    finalize(part, dest).await
}

/// Move the finished `.part` file to its final name.
async fn finalize(part: &str, dest: &str) -> Outcome {
    match tokio::fs::rename(part, dest).await {
        Ok(()) => Outcome::Completed,
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

fn friendly(e: &reqwest::Error) -> String {
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
