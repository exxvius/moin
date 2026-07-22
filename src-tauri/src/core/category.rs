//! Categories: named buckets a download can be filed into. A category matches on
//! two axes: its **sources** (how the download arrived — empty means any) and its
//! **triggers** (content conditions on the file). A download is claimed only when
//! the source is accepted and *every* trigger passes. When several categories
//! match, the lowest `order` wins.
//!
//! This module is Tauri-free and unit-tested. The engine owns the live list and
//! persists it as `categories.json`; the manual-add flow and (later) watch
//! sources feed [`Candidate`]s through [`categorize`].

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::task::filename_from_url;

/// How a download entered moin. Manual methods are live now; the watch methods
/// are carried so categories built today keep working when automation lands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AddMethodKind {
    ManualLink,
    ManualTorrent,
    /// Handed to moin by the browser extension. A distinct source so a category
    /// can target (or exclude) browser captures; categories with no source filter
    /// still match them like any other add.
    BrowserCapture,
    WatchFolder,
    WatchUrlFile,
}

/// A content condition on the file itself. All of a category's triggers must
/// pass for a match. How the download *arrived* is a separate axis — see
/// [`Category::sources`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Trigger {
    /// Matches when the file extension is any of these (case-insensitive, no dot).
    Extension { exts: Vec<String> },
    /// Matches when the size in bytes falls within [min, max] (either end open).
    /// Fails when the size isn't known yet (e.g. before a probe).
    Size { min: Option<u64>, max: Option<u64> },
    /// Matches when the URL matches any of these globs (`*`/`?`; no wildcard =
    /// substring).
    UrlPattern { patterns: Vec<String> },
    /// Matches when the filename matches any of these globs.
    NamePattern { patterns: Vec<String> },
}

/// A user-defined bucket plus the rules that file downloads into it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Category {
    pub id: String,
    pub name: String,
    /// An accent id from the UI's swatch set (purely cosmetic here).
    #[serde(default)]
    pub color: String,
    /// Optional icon id from the UI's icon set; `None` shows the color dot.
    #[serde(default)]
    pub icon: Option<String>,
    /// Optional destination override; `None` means the default download dir.
    #[serde(default)]
    pub save_dir: Option<String>,
    /// Which add-methods this category accepts. Empty means any source.
    #[serde(default)]
    pub sources: Vec<AddMethodKind>,
    /// Content conditions; a download must satisfy all of them to match.
    #[serde(default)]
    pub triggers: Vec<Trigger>,
    /// Automated sources only (later phase): download non-matching candidates
    /// uncategorized instead of skipping them. Ignored for manual adds.
    #[serde(default)]
    pub fallback_download: bool,
    /// Priority; lower wins when several categories match.
    #[serde(default)]
    pub order: i32,
}

/// A download being evaluated against the categories. Fields not yet known
/// (notably `size` before a probe) are `None`, and triggers over them don't pass.
#[derive(Debug, Clone)]
pub struct Candidate {
    pub url: String,
    pub add_method: AddMethodKind,
    pub filename: String,
    pub extension: Option<String>,
    pub size: Option<u64>,
}

impl Candidate {
    /// Build a candidate from just a URL and how it was added — everything the
    /// manual-add path knows before anything is downloaded.
    pub fn from_url(url: &str, add_method: AddMethodKind) -> Self {
        let filename = filename_from_url(url);
        let extension = extension_of(&filename);
        Self {
            url: url.to_string(),
            add_method,
            filename,
            extension,
            size: None,
        }
    }
}

impl Trigger {
    fn passes(&self, c: &Candidate) -> bool {
        match self {
            Trigger::Extension { exts } => match &c.extension {
                Some(ext) => exts
                    .iter()
                    .any(|e| e.trim_start_matches('.').eq_ignore_ascii_case(ext)),
                None => false,
            },
            Trigger::Size { min, max } => match c.size {
                Some(sz) => min.map_or(true, |m| sz >= m) && max.map_or(true, |m| sz <= m),
                None => false,
            },
            Trigger::UrlPattern { patterns } => patterns.iter().any(|p| glob_match(p, &c.url)),
            Trigger::NamePattern { patterns } => {
                patterns.iter().any(|p| glob_match(p, &c.filename))
            }
        }
    }
}

/// Whether a category claims a candidate. A category matches when its source
/// filter admits the candidate (empty = any source) and every content trigger
/// passes. A category with neither a source nor a trigger never auto-matches —
/// there's nothing to match on.
fn matches(cat: &Category, c: &Candidate) -> bool {
    if cat.sources.is_empty() && cat.triggers.is_empty() {
        return false;
    }
    if !cat.sources.is_empty() && !cat.sources.contains(&c.add_method) {
        return false;
    }
    cat.triggers.iter().all(|t| t.passes(c))
}

/// The id of the first category (by ascending `order`) that claims `c`, if any.
pub fn categorize(c: &Candidate, cats: &[Category]) -> Option<String> {
    let mut ordered: Vec<&Category> = cats.iter().collect();
    ordered.sort_by_key(|cat| cat.order);
    ordered
        .into_iter()
        .find(|cat| matches(cat, c))
        .map(|cat| cat.id.clone())
}

/// Lowercased file extension (no dot), or `None` when there isn't one.
fn extension_of(filename: &str) -> Option<String> {
    filename
        .rsplit_once('.')
        .map(|(_, e)| e.to_lowercase())
        .filter(|e| !e.is_empty())
}

/// Case-insensitive match. With `*`/`?` it's a glob; without either it's a
/// friendlier substring test (so `arxiv.org` matches any URL containing it).
fn glob_match(pattern: &str, text: &str) -> bool {
    let p = pattern.to_lowercase();
    let t = text.to_lowercase();
    if !p.contains('*') && !p.contains('?') {
        return t.contains(&p);
    }
    wildcard(p.as_bytes(), t.as_bytes())
}

/// Classic linear wildcard match with `*` (any run) and `?` (one byte),
/// backtracking on `*`. Operates on the already-lowercased bytes.
fn wildcard(pat: &[u8], text: &[u8]) -> bool {
    let (mut p, mut t) = (0usize, 0usize);
    let mut star: Option<usize> = None;
    let mut mark = 0usize;
    while t < text.len() {
        if p < pat.len() && (pat[p] == b'?' || pat[p] == text[t]) {
            p += 1;
            t += 1;
        } else if p < pat.len() && pat[p] == b'*' {
            star = Some(p);
            mark = t;
            p += 1;
        } else if let Some(sp) = star {
            p = sp + 1;
            mark += 1;
            t = mark;
        } else {
            return false;
        }
    }
    while p < pat.len() && pat[p] == b'*' {
        p += 1;
    }
    p == pat.len()
}

fn json_path(data_dir: &Path) -> PathBuf {
    data_dir.join("categories.json")
}

/// Load the saved categories, seeding a generic starter set on a fresh install
/// (when the file doesn't exist yet). Once the file is present — even as an empty
/// list because the user deleted everything — it's taken as-is and never
/// re-seeded.
pub fn load_or_seed(data_dir: &Path) -> Vec<Category> {
    if json_path(data_dir).exists() {
        return load(data_dir);
    }
    let seeded = defaults();
    save(&seeded, data_dir);
    seeded
}

/// Load the saved categories (empty when the file is missing or unreadable).
pub fn load(data_dir: &Path) -> Vec<Category> {
    fs::read_to_string(json_path(data_dir))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// The starter categories shipped on first run: broad, obvious buckets keyed on
/// file extension. Fully editable — they're ordinary records with fresh ids.
pub fn defaults() -> Vec<Category> {
    // Seeded with no color — they show their icon in neutral and don't tint
    // download cards until the user gives them a color.
    let mk = |order: i32, name: &str, icon: &str, exts: &[&str]| Category {
        id: uuid::Uuid::new_v4().to_string(),
        name: name.to_string(),
        color: String::new(),
        icon: Some(icon.to_string()),
        save_dir: None,
        sources: Vec::new(),
        triggers: vec![Trigger::Extension {
            exts: exts.iter().map(|s| s.to_string()).collect(),
        }],
        fallback_download: false,
        order,
    };
    vec![
        mk(
            0,
            "Video",
            "film",
            &[
                "mp4", "mkv", "avi", "mov", "webm", "m4v", "flv", "wmv", "mpg", "mpeg",
            ],
        ),
        mk(
            1,
            "Audio",
            "music",
            &["mp3", "flac", "wav", "aac", "ogg", "m4a", "wma", "opus"],
        ),
        mk(
            2,
            "Images",
            "image",
            &[
                "jpg", "jpeg", "png", "gif", "webp", "bmp", "svg", "tiff", "heic",
            ],
        ),
        mk(
            3,
            "Documents",
            "file-text",
            &[
                "pdf", "doc", "docx", "xls", "xlsx", "ppt", "pptx", "txt", "odt", "epub",
            ],
        ),
        mk(
            4,
            "Compressed",
            "archive",
            &["zip", "rar", "7z", "tar", "gz", "bz2", "xz", "iso"],
        ),
        mk(
            5,
            "Programs",
            "app-window",
            &["exe", "msi", "dmg", "pkg", "deb", "rpm", "appimage"],
        ),
    ]
}

/// Persist the categories as pretty JSON (best-effort).
pub fn save(cats: &[Category], data_dir: &Path) {
    if let Ok(text) = serde_json::to_string_pretty(cats) {
        let _ = fs::write(json_path(data_dir), text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cat(id: &str, order: i32, triggers: Vec<Trigger>) -> Category {
        Category {
            id: id.to_string(),
            name: id.to_string(),
            color: String::new(),
            icon: None,
            save_dir: None,
            sources: Vec::new(),
            triggers,
            fallback_download: false,
            order,
        }
    }

    fn manual(url: &str) -> Candidate {
        Candidate::from_url(url, AddMethodKind::ManualLink)
    }

    #[test]
    fn all_triggers_must_pass() {
        let c = cat(
            "papers",
            0,
            vec![
                Trigger::UrlPattern {
                    patterns: vec!["x.com".into()],
                },
                Trigger::Extension {
                    exts: vec!["pdf".into()],
                },
            ],
        );
        assert_eq!(
            categorize(&manual("https://x.com/a.pdf"), std::slice::from_ref(&c)),
            Some("papers".into())
        );
        // Same URL but wrong extension fails the AND.
        assert_eq!(categorize(&manual("https://x.com/a.zip"), &[c]), None);
    }

    #[test]
    fn source_filter_gates_the_match() {
        let mut c = cat(
            "torrents",
            0,
            vec![Trigger::Extension {
                exts: vec!["torrent".into()],
            }],
        );
        c.sources = vec![AddMethodKind::ManualTorrent];
        // A manual *link* is the wrong source, even though the extension matches.
        assert_eq!(
            categorize(&manual("https://x.com/a.torrent"), std::slice::from_ref(&c)),
            None
        );
        let torrent = Candidate::from_url("https://x.com/a.torrent", AddMethodKind::ManualTorrent);
        assert_eq!(categorize(&torrent, &[c]), Some("torrents".into()));
    }

    #[test]
    fn source_only_category_matches_any_file() {
        let mut c = cat("all-torrents", 0, vec![]);
        c.sources = vec![AddMethodKind::ManualTorrent];
        let torrent = Candidate::from_url(
            "https://x.com/whatever.torrent",
            AddMethodKind::ManualTorrent,
        );
        assert_eq!(categorize(&torrent, &[c]), Some("all-torrents".into()));
    }

    #[test]
    fn lowest_order_wins() {
        let pdfs = cat(
            "pdfs",
            10,
            vec![Trigger::Extension {
                exts: vec!["pdf".into()],
            }],
        );
        let arxiv = cat(
            "arxiv",
            1,
            vec![Trigger::UrlPattern {
                patterns: vec!["arxiv.org".into()],
            }],
        );
        // Both match; the lower order (arxiv=1) claims it.
        let got = categorize(&manual("https://arxiv.org/p/a.pdf"), &[pdfs, arxiv]);
        assert_eq!(got, Some("arxiv".into()));
    }

    #[test]
    fn extension_is_case_insensitive() {
        let c = cat(
            "pdfs",
            0,
            vec![Trigger::Extension {
                exts: vec!["PDF".into()],
            }],
        );
        assert_eq!(
            categorize(&manual("https://x.com/A.PdF"), &[c]),
            Some("pdfs".into())
        );
    }

    #[test]
    fn unknown_size_does_not_match() {
        let c = cat(
            "big",
            0,
            vec![Trigger::Size {
                min: Some(1_000),
                max: None,
            }],
        );
        // Manual candidate has no size yet, so a size trigger can't be confirmed.
        assert_eq!(categorize(&manual("https://x.com/a.bin"), &[c]), None);
    }

    #[test]
    fn size_within_range_matches() {
        let c = cat(
            "mid",
            0,
            vec![Trigger::Size {
                min: Some(100),
                max: Some(200),
            }],
        );
        let mut cand = manual("https://x.com/a.bin");
        cand.size = Some(150);
        assert_eq!(categorize(&cand, &[c]), Some("mid".into()));
    }

    #[test]
    fn empty_category_never_matches() {
        // No sources and no triggers: nothing to match on.
        let c = cat("empty", 0, vec![]);
        assert_eq!(categorize(&manual("https://x.com/a.pdf"), &[c]), None);
    }

    #[test]
    fn defaults_are_usable_and_match() {
        let seeds = defaults();
        assert!(!seeds.is_empty());
        // Unique ids, each with at least one trigger.
        let ids: std::collections::HashSet<_> = seeds.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids.len(), seeds.len());
        assert!(seeds.iter().all(|c| !c.triggers.is_empty()));
        // A .mp4 lands in Video.
        let got = categorize(&manual("https://x.com/clip.mp4"), &seeds);
        let video = seeds.iter().find(|c| c.name == "Video").unwrap();
        assert_eq!(got.as_deref(), Some(video.id.as_str()));
    }

    #[test]
    fn glob_and_substring_url_matching() {
        assert!(glob_match("arxiv.org", "https://arxiv.org/abs/1"));
        assert!(glob_match("*.pdf", "paper.pdf"));
        assert!(glob_match("a?c", "abc"));
        assert!(!glob_match("*.pdf", "paper.zip"));
    }
}
