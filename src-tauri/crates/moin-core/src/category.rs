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

use regex::Regex;
use serde::{Deserialize, Serialize};

use super::task::{filename_from_url, TorrentFile};

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
    /// Optional separate color for the icon (an accent id or "black"). Empty means
    /// the icon inherits `color`, so a category set before this existed is unchanged.
    #[serde(default)]
    pub icon_color: String,
    /// Optional separate color for the card effects — border, hover/select glow,
    /// and cursor-proximity glow. Empty inherits `color`. Independent of the icon
    /// color, so effects and icon can differ.
    #[serde(default)]
    pub effects_color: String,
    /// Optional icon id from the UI's icon set; `None` shows the color dot.
    #[serde(default)]
    pub icon: Option<String>,
    /// Hide this category's downloads from the "All categories" filter — they only
    /// show when the category is picked explicitly. Mirrors how archived tasks stay
    /// out of the "All" status filter.
    #[serde(default)]
    pub hidden_from_all: bool,
    /// Optional destination override; `None` means the default download dir.
    #[serde(default)]
    pub save_dir: Option<String>,
    /// Seed-limit override for torrents filed here. `None` inherits the global
    /// seed ratio limit; `Some(0.0)` seeds forever (no ratio cap); `Some(r)` stops
    /// seeding once the ratio reaches `r`.
    #[serde(default)]
    pub seed_ratio_limit: Option<f64>,
    /// Seed-time override, in minutes. `None` inherits the global limit; `Some(0)`
    /// means no time cap; `Some(m)` stops seeding `m` minutes after completion.
    #[serde(default)]
    pub seed_time_limit_mins: Option<u64>,
    /// Optional staging folder for in-progress downloads (torrent and HTTP). While
    /// downloading, the data lives here; on completion the finished content moves
    /// into `save_dir` (or the default folder). `None`/empty downloads straight
    /// into the final folder.
    #[serde(default)]
    pub incomplete_dir: Option<String>,
    /// Save a copy of each added torrent's `.torrent` file into this folder.
    /// `None`/empty keeps no copy. Torrent sources only.
    #[serde(default)]
    pub torrent_file_dir: Option<String>,
    /// When a torrent finishes, move its saved `.torrent` copy here — a separate
    /// home for completed torrents. `None`/empty leaves it in `torrent_file_dir`
    /// (or, if that's unset, exports a copy here on completion only).
    #[serde(default)]
    pub torrent_file_done_dir: Option<String>,
    /// Which add-methods this category accepts. Empty means any source.
    #[serde(default)]
    pub sources: Vec<AddMethodKind>,
    /// Folders watched for dropped `.torrent` files — a torrent found in one is
    /// added under this category (a source, configured in the editor's Sources
    /// section). Empty means the category watches nothing.
    #[serde(default)]
    pub watch_folders: Vec<String>,
    /// When a `.torrent` file downloaded through moin files into this category,
    /// re-add it as a torrent instead of leaving the downloaded file. A per-category
    /// source toggle; off keeps the plain `.torrent` download.
    #[serde(default)]
    pub capture_torrent_downloads: bool,
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
    /// What happens to a torrent once this category claims it — watch folders,
    /// file exclusions, layout, renames. Fully defaulted, so a category without
    /// automation behaves exactly as before and pre-automation JSON loads unchanged.
    #[serde(default)]
    pub automation: Automation,
}

/// Per-category automation: how a torrent filed into this category is organized
/// once claimed — regardless of how it arrived (manual, watch folder, or a
/// captured `.torrent` download). Every field defaults to empty/neutral, so a
/// plain category is a no-op here and old `categories.json` deserializes with
/// `automation` at its `Default`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Automation {
    /// A file matching any of these is deselected (never downloaded). A rule set
    /// that would drop *every* file is ignored, so a torrent is never nuked whole.
    pub exclude: Vec<FileMatch>,
    /// How the torrent's files land on disk relative to the save folder.
    pub layout: LayoutMode,
    /// Rename steps applied in order to each file's name (not its folder path).
    pub renames: Vec<RenameRule>,
}

/// A condition on a single torrent file, used by auto-exclude. The inverse of a
/// [`Trigger`]: where a trigger *admits* a download, a `FileMatch` *drops* a file.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum FileMatch {
    /// The file's extension is any of these (case-insensitive, no dot).
    Extension { exts: Vec<String> },
    /// The file's relative path matches any of these globs (`*`/`?`, or substring).
    NamePattern { patterns: Vec<String> },
    /// The file is smaller than this many bytes (kills tiny junk: samples, readmes).
    SizeUnder { bytes: u64 },
    /// The file is larger than this many bytes.
    SizeOver { bytes: u64 },
}

impl FileMatch {
    /// Whether this rule drops the file at `path` of `size` bytes.
    fn excludes(&self, path: &str, size: u64) -> bool {
        match self {
            FileMatch::Extension { exts } => match extension_of(path) {
                Some(ext) => exts
                    .iter()
                    .any(|e| e.trim_start_matches('.').eq_ignore_ascii_case(&ext)),
                None => false,
            },
            FileMatch::NamePattern { patterns } => patterns.iter().any(|p| glob_match(p, path)),
            FileMatch::SizeUnder { bytes } => size < *bytes,
            FileMatch::SizeOver { bytes } => size > *bytes,
        }
    }
}

/// How a torrent's files are placed relative to the chosen save folder. Mirrors
/// the add-torrent modal's layout choice.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LayoutMode {
    /// Keep the torrent's own structure — nests under a torrent-named folder for a
    /// multi-file torrent, saves a single-file torrent directly.
    #[default]
    Original,
    /// Always nest the content under a torrent-named subfolder.
    Subfolder,
    /// Save files straight into the folder, no subfolder.
    Flat,
}

/// One rename step. Steps apply in order, so several can compose (e.g. dots to
/// spaces, then strip tags). Only a file's name is rewritten; its folder is kept.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum RenameRule {
    /// Find/replace on the file name. With `regex`, `find` is a regular expression
    /// and `with` may reference capture groups (`$1`); otherwise both are literal.
    /// An invalid regex is skipped (the step becomes a no-op).
    Replace {
        find: String,
        with: String,
        #[serde(default)]
        regex: bool,
    },
    /// A canned transform applied to the name's stem (its extension is preserved).
    Preset { kind: PresetKind },
}

/// The canned rename transforms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PresetKind {
    /// `Movie.2020.1080p` → `Movie 2020 1080p`.
    DotsToSpaces,
    /// `Movie_2020_1080p` → `Movie 2020 1080p`.
    UnderscoresToSpaces,
    /// Lowercase the stem.
    Lowercase,
    /// Drop `[...]` / `(...)` / `{...}` bracketed groups.
    StripTags,
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
        Self::from_url_named(url, add_method, None)
    }

    /// Like [`from_url`] but with a filename the caller already knows (e.g. a
    /// browser capture's `download` attribute or a `Content-Disposition`), which
    /// then drives the name/extension triggers; falls back to the URL otherwise.
    pub fn from_url_named(url: &str, add_method: AddMethodKind, filename: Option<&str>) -> Self {
        let filename = filename
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| filename_from_url(url));
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
    /// Whether this trigger passes for a whole torrent (a *set* of files), used by
    /// watch-folder gating. A single-candidate trigger doesn't fit a torrent, so
    /// the axes are lifted: extension matches if *any* file has it, name matches
    /// the torrent name or any file path, and size uses the torrent's total. This
    /// makes a `mkv`-triggered category accept a movie torrent (whose *name* has no
    /// extension) rather than reject it.
    fn passes_torrent(&self, name: &str, total: u64, files: &[TorrentFile]) -> bool {
        match self {
            Trigger::Extension { exts } => files.iter().any(|f| match extension_of(&f.path) {
                Some(ext) => exts
                    .iter()
                    .any(|e| e.trim_start_matches('.').eq_ignore_ascii_case(&ext)),
                None => false,
            }),
            Trigger::Size { min, max } => {
                min.map_or(true, |m| total >= m) && max.map_or(true, |m| total <= m)
            }
            Trigger::UrlPattern { patterns } => patterns.iter().any(|p| glob_match(p, name)),
            Trigger::NamePattern { patterns } => {
                patterns.iter().any(|p| glob_match(p, name))
                    || files
                        .iter()
                        .any(|f| patterns.iter().any(|p| glob_match(p, &f.path)))
            }
        }
    }

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

/// The indices of `files` to download after applying `cat`'s exclude rules.
/// Returns an **empty vec** to mean "download everything" — either the category
/// has no exclusions, or the rules would drop every file (which we refuse, so a
/// torrent is never excluded whole). Otherwise the surviving indices, which the
/// add path maps onto per-file selection. `None` category = no automation = all.
pub fn auto_selection(cat: Option<&Category>, files: &[TorrentFile]) -> Vec<usize> {
    let Some(cat) = cat else { return Vec::new() };
    let rules = &cat.automation.exclude;
    if rules.is_empty() || files.is_empty() {
        return Vec::new();
    }
    let kept: Vec<usize> = files
        .iter()
        .enumerate()
        .filter(|(_, f)| !rules.iter().any(|r| r.excludes(&f.path, f.size)))
        .map(|(i, _)| i)
        .collect();
    // Nothing dropped, or everything dropped → the "all files" form.
    if kept.len() == files.len() || kept.is_empty() {
        Vec::new()
    } else {
        kept
    }
}

/// Whether a watched-folder category accepts a torrent dropped into it. A category
/// with no content triggers takes anything (the folder assignment is the intent);
/// otherwise every trigger must pass — evaluated across the torrent's files (see
/// [`Trigger::passes_torrent`]). Sources aren't checked: the folder already
/// belongs to this category.
pub fn watch_accepts(cat: &Category, name: &str, total: u64, files: &[TorrentFile]) -> bool {
    cat.triggers.is_empty()
        || cat
            .triggers
            .iter()
            .all(|t| t.passes_torrent(name, total, files))
}

/// The output subfolder implied by `cat`'s layout for a torrent named
/// `torrent_name` with `file_count` files: `Some(name)` nests the content under a
/// torrent-named folder, `None` saves it directly. Mirrors the modal's `nest`
/// rule (Original nests only a multi-file torrent). The name is sanitized by the
/// add path, so it's passed through raw here.
pub fn layout_folder(cat: &Category, torrent_name: &str, file_count: usize) -> Option<String> {
    let nest = match cat.automation.layout {
        LayoutMode::Subfolder => true,
        LayoutMode::Flat => false,
        LayoutMode::Original => file_count > 1,
    };
    nest.then(|| torrent_name.to_string())
}

/// The renamed relative path for each of `files` after applying `cat`'s rename
/// rules (index-aligned). A blank entry means "keep the original", which is what
/// [`crate::engine`]'s add path expects. Only the file name is rewritten;
/// its folder path is preserved.
pub fn plan_renames(cat: &Category, files: &[TorrentFile]) -> Vec<String> {
    let rules = &cat.automation.renames;
    if rules.is_empty() {
        return vec![String::new(); files.len()];
    }
    // Pre-compile any regex rules once, not per file. An invalid pattern drops to
    // a no-op so a typo can't break the whole add.
    let compiled: Vec<Option<Regex>> = rules
        .iter()
        .map(|r| match r {
            RenameRule::Replace {
                find, regex: true, ..
            } => Regex::new(find).ok(),
            _ => None,
        })
        .collect();

    files
        .iter()
        .map(|f| {
            let (dir, name) = split_dir_name(&f.path);
            let renamed = apply_renames(name, rules, &compiled);
            if renamed == name {
                String::new()
            } else if dir.is_empty() {
                renamed
            } else {
                format!("{dir}/{renamed}")
            }
        })
        .collect()
}

/// Run every rename step over one file name, in order, then tidy the result: the
/// stem's whitespace is collapsed and trimmed while the extension is reattached
/// cleanly, so a transform that leaves doubled or edge spaces (dots→spaces,
/// tag-stripping) doesn't strand a space against the extension dot.
fn apply_renames(name: &str, rules: &[RenameRule], compiled: &[Option<Regex>]) -> String {
    let mut out = name.to_string();
    for (i, rule) in rules.iter().enumerate() {
        out = match rule {
            RenameRule::Replace {
                with, regex: true, ..
            } => match &compiled[i] {
                Some(re) => re.replace_all(&out, with.as_str()).into_owned(),
                None => out, // invalid regex: no-op
            },
            RenameRule::Replace {
                find,
                with,
                regex: false,
            } => {
                if find.is_empty() {
                    out
                } else {
                    out.replace(find, with)
                }
            }
            RenameRule::Preset { kind } => apply_preset(*kind, &out),
        };
    }
    let (stem, ext) = split_ext(&out);
    let stem = collapse_spaces(stem);
    match ext {
        Some(ext) => format!("{stem}.{ext}"),
        None => stem,
    }
}

/// Apply a canned transform to a name's stem, preserving its extension.
fn apply_preset(kind: PresetKind, name: &str) -> String {
    let (stem, ext) = split_ext(name);
    let new_stem = match kind {
        PresetKind::DotsToSpaces => stem.replace('.', " "),
        PresetKind::UnderscoresToSpaces => stem.replace('_', " "),
        PresetKind::Lowercase => stem.to_lowercase(),
        PresetKind::StripTags => strip_tags(stem),
    };
    match ext {
        Some(ext) => format!("{new_stem}.{ext}"),
        None => new_stem,
    }
}

/// Remove `[...]` / `(...)` / `{...}` bracketed groups from a name.
fn strip_tags(stem: &str) -> String {
    let mut out = String::with_capacity(stem.len());
    let mut depth = 0i32;
    for c in stem.chars() {
        match c {
            '[' | '(' | '{' => depth += 1,
            ']' | ')' | '}' if depth > 0 => depth -= 1,
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out
}

/// Collapse runs of whitespace to single spaces and trim — tidies up after a
/// dots/tags transform that leaves doubled or trailing spaces.
fn collapse_spaces(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Split a `/`-separated relative path into its directory (without trailing slash)
/// and file name.
fn split_dir_name(path: &str) -> (&str, &str) {
    match path.rsplit_once('/') {
        Some((dir, name)) => (dir, name),
        None => ("", path),
    }
}

/// Split a file name into its stem and extension (no dot). No extension → `None`;
/// a leading-dot name like `.env` is treated as all stem.
fn split_ext(name: &str) -> (&str, Option<&str>) {
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() && !ext.is_empty() => (stem, Some(ext)),
        _ => (name, None),
    }
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
        icon_color: String::new(),
        effects_color: String::new(),
        icon: Some(icon.to_string()),
        hidden_from_all: false,
        save_dir: None,
        seed_ratio_limit: None,
        seed_time_limit_mins: None,
        incomplete_dir: None,
        torrent_file_dir: None,
        torrent_file_done_dir: None,
        sources: Vec::new(),
        watch_folders: Vec::new(),
        capture_torrent_downloads: false,
        triggers: vec![Trigger::Extension {
            exts: exts.iter().map(|s| s.to_string()).collect(),
        }],
        fallback_download: false,
        order,
        automation: Automation::default(),
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
            icon_color: String::new(),
            effects_color: String::new(),
            icon: None,
            hidden_from_all: false,
            save_dir: None,
            seed_ratio_limit: None,
            seed_time_limit_mins: None,
            incomplete_dir: None,
            torrent_file_dir: None,
            torrent_file_done_dir: None,
            sources: Vec::new(),
            watch_folders: Vec::new(),
            capture_torrent_downloads: false,
            triggers,
            fallback_download: false,
            order,
            automation: Automation::default(),
        }
    }

    fn tf(path: &str, size: u64) -> TorrentFile {
        TorrentFile {
            path: path.to_string(),
            size,
            selected: true,
            received: 0,
        }
    }

    /// A category carrying only the given automation (no matching rules).
    fn auto_cat(automation: Automation) -> Category {
        let mut c = cat("auto", 0, vec![]);
        c.automation = automation;
        c
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

    // --- automation: auto-exclude ---

    #[test]
    fn no_exclusions_downloads_everything() {
        let files = vec![tf("a.mkv", 100), tf("b.nfo", 10)];
        assert_eq!(auto_selection(None, &files), Vec::<usize>::new());
        let c = auto_cat(Automation::default());
        assert_eq!(auto_selection(Some(&c), &files), Vec::<usize>::new());
    }

    #[test]
    fn excludes_by_extension_and_name_and_size() {
        let c = auto_cat(Automation {
            exclude: vec![
                FileMatch::Extension {
                    exts: vec!["nfo".into(), "txt".into()],
                },
                FileMatch::NamePattern {
                    patterns: vec!["*sample*".into()],
                },
                FileMatch::SizeUnder { bytes: 50 },
            ],
            ..Automation::default()
        });
        let files = vec![
            tf("movie.mkv", 1_000),      // 0 keep
            tf("info.nfo", 500),         // 1 drop (ext)
            tf("Sample/clip.mkv", 800),  // 2 drop (name glob on path)
            tf("readme.txt", 10),        // 3 drop (ext + size)
            tf("tiny.dat", 20),          // 4 drop (size)
            tf("extra.mkv", 900),        // 5 keep
        ];
        assert_eq!(auto_selection(Some(&c), &files), vec![0, 5]);
    }

    #[test]
    fn excluding_every_file_is_refused() {
        // A rule that would drop all files falls back to "download everything"
        // rather than leaving nothing to fetch.
        let c = auto_cat(Automation {
            exclude: vec![FileMatch::Extension {
                exts: vec!["mkv".into()],
            }],
            ..Automation::default()
        });
        let files = vec![tf("a.mkv", 1), tf("b.mkv", 2)];
        assert_eq!(auto_selection(Some(&c), &files), Vec::<usize>::new());
    }

    #[test]
    fn size_over_drops_large_files() {
        let c = auto_cat(Automation {
            exclude: vec![FileMatch::SizeOver { bytes: 1000 }],
            ..Automation::default()
        });
        let files = vec![tf("small.bin", 500), tf("huge.bin", 5000)];
        assert_eq!(auto_selection(Some(&c), &files), vec![0]);
    }

    // --- automation: layout ---

    #[test]
    fn layout_folder_follows_the_mode() {
        let flat = auto_cat(Automation {
            layout: LayoutMode::Flat,
            ..Automation::default()
        });
        assert_eq!(layout_folder(&flat, "Some.Torrent", 3), None);

        let sub = auto_cat(Automation {
            layout: LayoutMode::Subfolder,
            ..Automation::default()
        });
        assert_eq!(
            layout_folder(&sub, "Some.Torrent", 1).as_deref(),
            Some("Some.Torrent")
        );

        let orig = auto_cat(Automation::default());
        // Original nests only a multi-file torrent.
        assert_eq!(layout_folder(&orig, "T", 1), None);
        assert_eq!(layout_folder(&orig, "T", 2).as_deref(), Some("T"));
    }

    // --- automation: watch gate ---

    #[test]
    fn watch_accepts_anything_without_triggers() {
        let c = cat("watch", 0, vec![]);
        let files = vec![tf("Movie/clip.mkv", 100)];
        assert!(watch_accepts(&c, "Movie", 100, &files));
    }

    #[test]
    fn watch_gates_extension_across_files() {
        // A movie category triggered on video extensions must accept a torrent
        // whose *name* has no extension but whose files do.
        let c = cat(
            "movies",
            0,
            vec![Trigger::Extension {
                exts: vec!["mkv".into(), "mp4".into()],
            }],
        );
        let video = vec![tf("Some.Film.2020/movie.mkv", 900), tf("readme.nfo", 10)];
        let music = vec![tf("Album/track.flac", 50)];
        assert!(watch_accepts(&c, "Some.Film.2020", 910, &video));
        assert!(!watch_accepts(&c, "Album", 50, &music));
    }

    #[test]
    fn watch_gates_on_name_and_size() {
        let c = cat(
            "hd",
            0,
            vec![
                Trigger::NamePattern {
                    patterns: vec!["*1080p*".into()],
                },
                Trigger::Size {
                    min: Some(500),
                    max: None,
                },
            ],
        );
        let files = vec![tf("x.mkv", 800)];
        assert!(watch_accepts(&c, "Film.1080p", 800, &files));
        assert!(!watch_accepts(&c, "Film.480p", 800, &files)); // name fails
        assert!(!watch_accepts(&c, "Film.1080p", 100, &files)); // size fails
    }

    // --- automation: rename ---

    #[test]
    fn no_rename_rules_keep_original() {
        let c = auto_cat(Automation::default());
        let files = vec![tf("a/b.mkv", 1)];
        assert_eq!(plan_renames(&c, &files), vec![String::new()]);
    }

    #[test]
    fn preset_dots_to_spaces_keeps_extension_and_folder() {
        let c = auto_cat(Automation {
            renames: vec![RenameRule::Preset {
                kind: PresetKind::DotsToSpaces,
            }],
            ..Automation::default()
        });
        let files = vec![tf("Show.S01/Ep.01.1080p.mkv", 1)];
        assert_eq!(plan_renames(&c, &files), vec!["Show.S01/Ep 01 1080p.mkv"]);
    }

    #[test]
    fn preset_strip_tags_removes_bracket_groups() {
        let c = auto_cat(Automation {
            renames: vec![
                RenameRule::Preset {
                    kind: PresetKind::StripTags,
                },
                RenameRule::Preset {
                    kind: PresetKind::DotsToSpaces,
                },
            ],
            ..Automation::default()
        });
        let files = vec![tf("[Group] Film.2020 (x265).mkv", 1)];
        assert_eq!(plan_renames(&c, &files), vec!["Film 2020.mkv"]);
    }

    #[test]
    fn literal_and_regex_replace() {
        let literal = auto_cat(Automation {
            renames: vec![RenameRule::Replace {
                find: ".WEBRip".into(),
                with: "".into(),
                regex: false,
            }],
            ..Automation::default()
        });
        let files = vec![tf("Film.WEBRip.mkv", 1)];
        assert_eq!(plan_renames(&literal, &files), vec!["Film.mkv"]);

        // `${1}` (not `$1p`, which the regex crate reads as a group named "1p").
        let re = auto_cat(Automation {
            renames: vec![RenameRule::Replace {
                find: r"\.(\d{3,4})p".into(),
                with: " [${1}p]".into(),
                regex: true,
            }],
            ..Automation::default()
        });
        let files = vec![tf("Film.1080p.mkv", 1)];
        assert_eq!(plan_renames(&re, &files), vec!["Film [1080p].mkv"]);
    }

    #[test]
    fn invalid_regex_is_a_noop() {
        let c = auto_cat(Automation {
            renames: vec![RenameRule::Replace {
                find: "([".into(), // won't compile
                with: "x".into(),
                regex: true,
            }],
            ..Automation::default()
        });
        let files = vec![tf("keep.mkv", 1)];
        assert_eq!(plan_renames(&c, &files), vec![String::new()]);
    }

    #[test]
    fn automation_defaults_when_absent_from_json() {
        // Pre-automation categories.json has no `automation` key — it must load
        // with the field defaulted, not fail.
        let json = r#"[{"id":"x","name":"X","color":"","icon":null,"save_dir":null,
            "sources":[],"triggers":[],"fallback_download":false,"order":0}]"#;
        let cats: Vec<Category> = serde_json::from_str(json).unwrap();
        assert_eq!(cats.len(), 1);
        assert!(cats[0].watch_folders.is_empty());
        assert!(!cats[0].capture_torrent_downloads);
        assert!(cats[0].automation.exclude.is_empty());
        assert_eq!(cats[0].automation.layout, LayoutMode::Original);
    }
}
