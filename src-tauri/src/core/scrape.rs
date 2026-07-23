//! Tracker scrape (BEP 48 over HTTP, BEP 15 over UDP) — the swarm's seeder /
//! leecher counts, which librqbit doesn't surface. We ask a torrent's trackers
//! directly and keep the best (highest) answer, the way qBittorrent does.

use std::net::SocketAddr;
use std::time::Duration;

use tokio::net::UdpSocket;

/// One tracker's scrape reply for a single info hash.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ScrapeCounts {
    /// Complete peers (seeders).
    pub seeders: u32,
    /// Incomplete peers (leechers).
    pub leechers: u32,
    /// Times the torrent has been completed, if the tracker reports it.
    pub downloaded: u32,
}

/// Per-request timeout. Trackers that don't answer quickly are skipped.
const TIMEOUT: Duration = Duration::from_secs(5);

/// Scrape every tracker for `info_hash` concurrently and return the best counts
/// seen (max seeders, max leechers). `None` if no tracker answered.
pub async fn scrape_best(
    client: &reqwest::Client,
    trackers: &[String],
    info_hash: &[u8; 20],
) -> Option<ScrapeCounts> {
    let futures = trackers.iter().map(|t| scrape_one(client, t, info_hash));
    let results = futures_util::future::join_all(futures).await;

    let mut best: Option<ScrapeCounts> = None;
    for counts in results.into_iter().flatten() {
        best = Some(match best {
            None => counts,
            Some(b) => ScrapeCounts {
                seeders: b.seeders.max(counts.seeders),
                leechers: b.leechers.max(counts.leechers),
                downloaded: b.downloaded.max(counts.downloaded),
            },
        });
    }
    best
}

/// Scrape a single tracker (HTTP or UDP), returning `None` on any failure.
async fn scrape_one(
    client: &reqwest::Client,
    tracker: &str,
    info_hash: &[u8; 20],
) -> Option<ScrapeCounts> {
    let lower = tracker.trim().to_ascii_lowercase();
    let result = if lower.starts_with("udp://") {
        tokio::time::timeout(TIMEOUT, scrape_udp(tracker, info_hash))
            .await
            .ok()?
    } else if lower.starts_with("http://") || lower.starts_with("https://") {
        tokio::time::timeout(TIMEOUT, scrape_http(client, tracker, info_hash))
            .await
            .ok()?
    } else {
        None
    };
    result
}

// ---- HTTP scrape (BEP 48) ----

/// The scrape URL for an announce URL: the text after the final `/` must start
/// with `announce`; replace that prefix with `scrape`. Trackers whose announce
/// path doesn't start with `announce` don't support scrape.
fn scrape_url(announce: &str) -> Option<String> {
    let slash = announce.rfind('/')?;
    let (base, last) = announce.split_at(slash + 1);
    let rest = last.strip_prefix("announce")?;
    Some(format!("{base}scrape{rest}"))
}

async fn scrape_http(
    client: &reqwest::Client,
    announce: &str,
    info_hash: &[u8; 20],
) -> Option<ScrapeCounts> {
    let base = scrape_url(announce)?;
    let sep = if base.contains('?') { '&' } else { '?' };
    let url = format!("{base}{sep}info_hash={}", percent_encode(info_hash));

    let body = client.get(&url).send().await.ok()?.bytes().await.ok()?;
    let value = bencode::parse(&body)?;
    let files = value.dict_get("files")?;
    // The single info-hash entry — take the first (and only) file record.
    let entry = files.first_dict_value()?;
    Some(ScrapeCounts {
        seeders: entry
            .dict_get("complete")
            .and_then(Bencode::int)
            .unwrap_or(0) as u32,
        leechers: entry
            .dict_get("incomplete")
            .and_then(Bencode::int)
            .unwrap_or(0) as u32,
        downloaded: entry
            .dict_get("downloaded")
            .and_then(Bencode::int)
            .unwrap_or(0) as u32,
    })
}

/// Percent-encode 20 raw bytes for a query string — every byte as `%XX`, always
/// valid regardless of the byte values.
fn percent_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 3);
    for b in bytes {
        s.push('%');
        s.push_str(&format!("{b:02x}"));
    }
    s
}

// ---- UDP scrape (BEP 15) ----

const UDP_PROTOCOL_ID: i64 = 0x0417_2710_1980;
const ACTION_CONNECT: i32 = 0;
const ACTION_SCRAPE: i32 = 2;

async fn scrape_udp(tracker: &str, info_hash: &[u8; 20]) -> Option<ScrapeCounts> {
    // udp://host:port/path -> host:port
    let host = tracker.strip_prefix("udp://")?;
    let host = host.split('/').next()?;
    let addr = tokio::net::lookup_host(host).await.ok()?.next()?;

    let bind: SocketAddr = if addr.is_ipv6() {
        "[::]:0".parse().ok()?
    } else {
        "0.0.0.0:0".parse().ok()?
    };
    let sock = UdpSocket::bind(bind).await.ok()?;
    sock.connect(addr).await.ok()?;

    // 1) Connect handshake.
    let txn: i32 = (now_nanos() as i32) ^ 0x5f3a_c71d;
    let mut req = Vec::with_capacity(16);
    req.extend_from_slice(&UDP_PROTOCOL_ID.to_be_bytes());
    req.extend_from_slice(&ACTION_CONNECT.to_be_bytes());
    req.extend_from_slice(&txn.to_be_bytes());
    sock.send(&req).await.ok()?;

    let mut buf = [0u8; 1024];
    let n = sock.recv(&mut buf).await.ok()?;
    if n < 16 || i32::from_be_bytes(buf[0..4].try_into().ok()?) != ACTION_CONNECT {
        return None;
    }
    if i32::from_be_bytes(buf[4..8].try_into().ok()?) != txn {
        return None;
    }
    let connection_id = i64::from_be_bytes(buf[8..16].try_into().ok()?);

    // 2) Scrape request for the single info hash.
    let txn2: i32 = txn.wrapping_add(1);
    let mut req = Vec::with_capacity(36);
    req.extend_from_slice(&connection_id.to_be_bytes());
    req.extend_from_slice(&ACTION_SCRAPE.to_be_bytes());
    req.extend_from_slice(&txn2.to_be_bytes());
    req.extend_from_slice(info_hash);
    sock.send(&req).await.ok()?;

    let n = sock.recv(&mut buf).await.ok()?;
    // 8-byte header + 12 bytes per hash (seeders, completed, leechers).
    if n < 20 || i32::from_be_bytes(buf[0..4].try_into().ok()?) != ACTION_SCRAPE {
        return None;
    }
    if i32::from_be_bytes(buf[4..8].try_into().ok()?) != txn2 {
        return None;
    }
    let seeders = u32::from_be_bytes(buf[8..12].try_into().ok()?);
    let downloaded = u32::from_be_bytes(buf[12..16].try_into().ok()?);
    let leechers = u32::from_be_bytes(buf[16..20].try_into().ok()?);
    Some(ScrapeCounts {
        seeders,
        leechers,
        downloaded,
    })
}

fn now_nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

// ---- Minimal bencode reader (enough to walk a scrape response) ----

mod bencode {
    /// A bencode value. Strings hold raw bytes; dict keys are UTF-8 lossy for
    /// lookup. Only what a scrape response needs.
    pub enum Bencode<'a> {
        Int(i64),
        Bytes(&'a [u8]),
        // Parsed so the reader can skip past lists; contents are never inspected.
        List(#[allow(dead_code)] Vec<Bencode<'a>>),
        Dict(Vec<(&'a [u8], Bencode<'a>)>),
    }

    pub fn parse(input: &[u8]) -> Option<Bencode<'_>> {
        let (v, _) = parse_at(input, 0)?;
        Some(v)
    }

    fn parse_at(b: &[u8], i: usize) -> Option<(Bencode<'_>, usize)> {
        match b.get(i)? {
            b'i' => {
                let end = find(b, i + 1, b'e')?;
                let n: i64 = std::str::from_utf8(&b[i + 1..end]).ok()?.parse().ok()?;
                Some((Bencode::Int(n), end + 1))
            }
            b'0'..=b'9' => {
                let colon = find(b, i, b':')?;
                let len: usize = std::str::from_utf8(&b[i..colon]).ok()?.parse().ok()?;
                let start = colon + 1;
                let end = start + len;
                if end > b.len() {
                    return None;
                }
                Some((Bencode::Bytes(&b[start..end]), end))
            }
            b'l' => {
                let mut items = Vec::new();
                let mut j = i + 1;
                while *b.get(j)? != b'e' {
                    let (v, next) = parse_at(b, j)?;
                    items.push(v);
                    j = next;
                }
                Some((Bencode::List(items), j + 1))
            }
            b'd' => {
                let mut pairs = Vec::new();
                let mut j = i + 1;
                while *b.get(j)? != b'e' {
                    let (key, next) = parse_at(b, j)?;
                    let Bencode::Bytes(k) = key else { return None };
                    let (val, next2) = parse_at(b, next)?;
                    pairs.push((k, val));
                    j = next2;
                }
                Some((Bencode::Dict(pairs), j + 1))
            }
            _ => None,
        }
    }

    fn find(b: &[u8], from: usize, target: u8) -> Option<usize> {
        (from..b.len()).find(|&k| b[k] == target)
    }

    impl<'a> Bencode<'a> {
        pub fn int(&self) -> Option<i64> {
            match self {
                Bencode::Int(n) => Some(*n),
                _ => None,
            }
        }

        pub fn dict_get(&self, key: &str) -> Option<&Bencode<'a>> {
            match self {
                Bencode::Dict(pairs) => pairs
                    .iter()
                    .find(|(k, _)| *k == key.as_bytes())
                    .map(|(_, v)| v),
                _ => None,
            }
        }

        /// The first value of a dict — used to grab the single file record in a
        /// scrape `files` dict without knowing the exact info-hash key bytes.
        pub fn first_dict_value(&self) -> Option<&Bencode<'a>> {
            match self {
                Bencode::Dict(pairs) => pairs.first().map(|(_, v)| v),
                _ => None,
            }
        }
    }
}

use bencode::Bencode;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derives_scrape_url_from_announce() {
        assert_eq!(
            scrape_url("http://t.example/announce").as_deref(),
            Some("http://t.example/scrape")
        );
        assert_eq!(
            scrape_url("https://t.example/x/announce.php?p=1").as_deref(),
            Some("https://t.example/x/scrape.php?p=1")
        );
        // No "announce" in the final path segment -> scrape unsupported.
        assert_eq!(scrape_url("http://t.example/track"), None);
    }

    #[test]
    fn parses_a_scrape_response() {
        // d5:filesd20:<hash>d8:completei5e10:downloadedi42e10:incompletei3eeee
        let mut resp = Vec::new();
        resp.extend_from_slice(b"d5:filesd20:");
        resp.extend_from_slice(&[0u8; 20]);
        resp.extend_from_slice(b"d8:completei5e10:downloadedi42e10:incompletei3eeee");
        let value = bencode::parse(&resp).unwrap();
        let entry = value.dict_get("files").unwrap().first_dict_value().unwrap();
        assert_eq!(entry.dict_get("complete").and_then(Bencode::int), Some(5));
        assert_eq!(entry.dict_get("incomplete").and_then(Bencode::int), Some(3));
        assert_eq!(
            entry.dict_get("downloaded").and_then(Bencode::int),
            Some(42)
        );
    }

    #[test]
    fn percent_encodes_bytes() {
        assert_eq!(percent_encode(&[0x00, 0xff, 0x41]), "%00%ff%41");
    }
}
