// OWNER: analysis
//! Unified diff parser + changed-line index used for anchoring findings.
//!
//! `parse_unified` understands `git diff` output (and plain `diff -u` output): several files, renames
//! and copies (`rename from` / `rename to`), new and deleted files (`/dev/null`, `new file mode`,
//! `deleted file mode`), binary markers (`Binary files … differ`, `GIT binary patch`), C-quoted paths,
//! `\ No newline at end of file`, and hunk headers with or without line counts.
//!
//! Path conventions: `FileDiff::old_path` and `new_path` are always real paths (never `/dev/null`).
//! For a new file both hold the new path, for a deleted file both hold the old path; `is_new` /
//! `is_deleted` say which. `ChangedIndex` is keyed by `new_path`.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::OnceLock;

use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use regex::Regex;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::types::DiffPosition;

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../ui/src/bindings/")]
pub struct FileDiff {
    pub old_path: String,
    pub new_path: String,
    pub is_new: bool,
    pub is_deleted: bool,
    pub is_binary: bool,
    pub hunks: Vec<Hunk>,
}

impl FileDiff {
    /// Iterates over every added line `(new_line, text)` of this file.
    pub fn added_lines(&self) -> impl Iterator<Item = (u32, &str)> + '_ {
        self.hunks.iter().flat_map(|h| {
            h.lines.iter().filter_map(|l| match (l.kind, l.new_line) {
                (LineKind::Add, Some(n)) => Some((n, l.text.as_str())),
                _ => None,
            })
        })
    }

    /// Number of added and deleted lines.
    pub fn stats(&self) -> (u32, u32) {
        let mut add = 0;
        let mut del = 0;
        for l in self.hunks.iter().flat_map(|h| h.lines.iter()) {
            match l.kind {
                LineKind::Add => add += 1,
                LineKind::Del => del += 1,
                LineKind::Context => {}
            }
        }
        (add, del)
    }

    /// True if the file was renamed (or copied) without being new/deleted.
    pub fn is_rename(&self) -> bool {
        self.old_path != self.new_path && !self.is_new && !self.is_deleted
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../ui/src/bindings/")]
pub struct Hunk {
    pub old_start: u32,
    pub old_lines: u32,
    pub new_start: u32,
    pub new_lines: u32,
    /// The `@@ -a,b +c,d @@ section` line (including any trailing section text).
    pub header: String,
    pub lines: Vec<DiffLine>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../ui/src/bindings/")]
pub struct DiffLine {
    pub kind: LineKind,
    pub old_line: Option<u32>,
    pub new_line: Option<u32>,
    /// Line content without the leading `+`/`-`/` ` marker.
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, export_to = "../../ui/src/bindings/")]
pub enum LineKind {
    Add,
    Del,
    Context,
}

// ---------------------------------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------------------------------

fn hunk_header_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@ ?(.*)$")
            .expect("hunk header regex")
    })
}

/// Paths of one file section while it is being parsed.
#[derive(Default)]
struct Builder {
    header_old: Option<String>,
    header_new: Option<String>,
    /// `Some(None)` = `/dev/null`.
    minus: Option<Option<String>>,
    plus: Option<Option<String>>,
    rename_from: Option<String>,
    rename_to: Option<String>,
    binary_old: Option<Option<String>>,
    binary_new: Option<Option<String>>,
    new_mode: bool,
    deleted_mode: bool,
    is_binary: bool,
    hunks: Vec<Hunk>,
}

impl Builder {
    fn finish(self) -> Option<FileDiff> {
        let dev_null_old =
            matches!(self.minus, Some(None)) || matches!(self.binary_old, Some(None));
        let dev_null_new = matches!(self.plus, Some(None)) || matches!(self.binary_new, Some(None));
        let is_new = self.new_mode || dev_null_old;
        let is_deleted = !is_new && (self.deleted_mode || dev_null_new);

        let old = self
            .rename_from
            .or(self.minus.flatten())
            .or(self.binary_old.flatten())
            .or(self.header_old);
        let new = self
            .rename_to
            .or(self.plus.flatten())
            .or(self.binary_new.flatten())
            .or(self.header_new);

        let (old_path, new_path) = match (old, new) {
            (Some(o), Some(n)) => {
                if is_new {
                    (n.clone(), n)
                } else if is_deleted {
                    (o.clone(), o)
                } else {
                    (o, n)
                }
            }
            (Some(o), None) => (o.clone(), o),
            (None, Some(n)) => (n.clone(), n),
            (None, None) => return None,
        };
        Some(FileDiff {
            old_path,
            new_path,
            is_new,
            is_deleted,
            is_binary: self.is_binary,
            hunks: self.hunks,
        })
    }

    fn has_paths_from_markers(&self) -> bool {
        self.minus.is_some() || self.plus.is_some() || !self.hunks.is_empty()
    }
}

/// State of the hunk currently being filled.
struct OpenHunk {
    hunk: Hunk,
    old_rem: u32,
    new_rem: u32,
    old_no: u32,
    new_no: u32,
}

impl OpenHunk {
    fn open(&self) -> bool {
        self.old_rem > 0 || self.new_rem > 0
    }
}

/// Parses `git diff` unified output (handles renames, new/deleted files, binary markers,
/// `\ No newline at end of file`).
pub fn parse_unified(diff: &str) -> Vec<FileDiff> {
    let mut files: Vec<FileDiff> = Vec::new();
    let mut cur: Option<Builder> = None;
    let mut hunk: Option<OpenHunk> = None;
    // True while `cur` was opened by a `diff --git` line and has not yet seen hunks — `---`/`+++`
    // then belong to it instead of starting a new (plain unified diff) section.
    let mut git_section = false;

    fn close_hunk(cur: &mut Option<Builder>, hunk: &mut Option<OpenHunk>) {
        if let Some(h) = hunk.take() {
            if let Some(b) = cur.as_mut() {
                b.hunks.push(h.hunk);
            }
        }
    }
    fn close_file(
        files: &mut Vec<FileDiff>,
        cur: &mut Option<Builder>,
        hunk: &mut Option<OpenHunk>,
    ) {
        close_hunk(cur, hunk);
        if let Some(b) = cur.take() {
            if let Some(f) = b.finish() {
                files.push(f);
            }
        }
    }

    for line in diff.lines() {
        // ---- inside a hunk: counts tell us how many body lines remain -----------------------
        if let Some(h) = hunk.as_mut() {
            if h.open() {
                let (marker, text) = match line.chars().next() {
                    Some(c) => (Some(c), &line[c.len_utf8()..]),
                    None => (None, ""),
                };
                match marker {
                    Some('+') if h.new_rem > 0 => {
                        h.hunk.lines.push(DiffLine {
                            kind: LineKind::Add,
                            old_line: None,
                            new_line: Some(h.new_no),
                            text: text.to_string(),
                        });
                        h.new_no += 1;
                        h.new_rem -= 1;
                        continue;
                    }
                    Some('-') if h.old_rem > 0 => {
                        h.hunk.lines.push(DiffLine {
                            kind: LineKind::Del,
                            old_line: Some(h.old_no),
                            new_line: None,
                            text: text.to_string(),
                        });
                        h.old_no += 1;
                        h.old_rem -= 1;
                        continue;
                    }
                    // A context line; an entirely empty line is a context line whose single space
                    // was stripped by an editor or mail client.
                    Some(' ') | None if h.old_rem > 0 && h.new_rem > 0 => {
                        h.hunk.lines.push(DiffLine {
                            kind: LineKind::Context,
                            old_line: Some(h.old_no),
                            new_line: Some(h.new_no),
                            text: text.to_string(),
                        });
                        h.old_no += 1;
                        h.new_no += 1;
                        h.old_rem -= 1;
                        h.new_rem -= 1;
                        continue;
                    }
                    Some('\\') => continue, // "\ No newline at end of file"
                    _ => {
                        // Malformed / truncated hunk: close it and treat the line as a header line.
                        close_hunk(&mut cur, &mut hunk);
                    }
                }
            }
        }

        // ---- header territory --------------------------------------------------------------
        if line.starts_with('\\') {
            continue; // "\ No newline at end of file" right after the last hunk line
        }

        if let Some(rest) = line.strip_prefix("diff --git ") {
            close_file(&mut files, &mut cur, &mut hunk);
            let mut b = Builder::default();
            if let Some((a, bb)) = split_git_header_paths(rest) {
                b.header_old = Some(strip_prefix_ab(&a, "a/"));
                b.header_new = Some(strip_prefix_ab(&bb, "b/"));
            }
            cur = Some(b);
            git_section = true;
            continue;
        }

        if let Some(caps) = hunk_header_re().captures(line) {
            close_hunk(&mut cur, &mut hunk);
            let num = |i: usize, default: u32| {
                caps.get(i)
                    .and_then(|m| m.as_str().parse::<u32>().ok())
                    .unwrap_or(default)
            };
            let old_start = num(1, 0);
            let old_lines = num(2, 1);
            let new_start = num(3, 0);
            let new_lines = num(4, 1);
            if cur.is_none() {
                cur = Some(Builder::default());
            }
            git_section = false;
            hunk = Some(OpenHunk {
                hunk: Hunk {
                    old_start,
                    old_lines,
                    new_start,
                    new_lines,
                    header: line.to_string(),
                    lines: Vec::new(),
                },
                old_rem: old_lines,
                new_rem: new_lines,
                // A side with zero lines starts at 0 (`@@ -0,0 +1,3 @@`); numbering is unused then.
                old_no: old_start.max(1),
                new_no: new_start.max(1),
            });
            if old_lines == 0 && new_lines == 0 {
                close_hunk(&mut cur, &mut hunk);
            }
            continue;
        }

        if let Some(rest) = line.strip_prefix("--- ") {
            // A `---` after hunks (or with no section at all) starts a new plain-diff file.
            let starts_new = match cur.as_ref() {
                None => true,
                Some(b) => !git_section && b.has_paths_from_markers(),
            };
            if starts_new {
                close_file(&mut files, &mut cur, &mut hunk);
                cur = Some(Builder::default());
            }
            if let Some(b) = cur.as_mut() {
                b.minus = Some(parse_marker_path(rest, "a/"));
            }
            continue;
        }

        if let Some(rest) = line.strip_prefix("+++ ") {
            if cur.is_none() {
                cur = Some(Builder::default());
            }
            if let Some(b) = cur.as_mut() {
                b.plus = Some(parse_marker_path(rest, "b/"));
            }
            continue;
        }

        let Some(b) = cur.as_mut() else { continue };
        if line.starts_with("new file mode") {
            b.new_mode = true;
        } else if line.starts_with("deleted file mode") {
            b.deleted_mode = true;
        } else if let Some(p) = line
            .strip_prefix("rename from ")
            .or_else(|| line.strip_prefix("copy from "))
        {
            b.rename_from = Some(unquote_path(p));
        } else if let Some(p) = line
            .strip_prefix("rename to ")
            .or_else(|| line.strip_prefix("copy to "))
        {
            b.rename_to = Some(unquote_path(p));
        } else if let Some(rest) = line.strip_prefix("Binary files ") {
            b.is_binary = true;
            if let Some(body) = rest.strip_suffix(" differ") {
                if let Some((o, n)) = body.split_once(" and ") {
                    b.binary_old = Some(dev_null_or(o, "a/"));
                    b.binary_new = Some(dev_null_or(n, "b/"));
                }
            }
        } else if line.starts_with("GIT binary patch") {
            b.is_binary = true;
        }
        // index / similarity / old mode / new mode / binary payload lines are ignored.
    }

    close_file(&mut files, &mut cur, &mut hunk);
    files
}

/// `"a/x b/y"` (optionally C-quoted) -> `("a/x", "b/y")`.
fn split_git_header_paths(rest: &str) -> Option<(String, String)> {
    let rest = rest.trim_end();
    if rest.starts_with('"') {
        let (a, tail) = take_quoted(rest)?;
        let tail = tail.trim_start();
        let b = if tail.starts_with('"') {
            take_quoted(tail)?.0
        } else {
            tail.to_string()
        };
        return Some((a, b));
    }
    if let Some(idx) = rest.find(" \"") {
        // unquoted a, quoted b
        let (a, tail) = rest.split_at(idx);
        let b = take_quoted(tail.trim_start())?.0;
        return Some((a.to_string(), b));
    }
    // Symmetric split: "a/P b/P" — prefer the " b/" whose two halves match.
    let positions: Vec<usize> = rest.match_indices(" b/").map(|(i, _)| i).collect();
    for &pos in &positions {
        let a = &rest[..pos];
        let b = &rest[pos + 1..];
        if a.strip_prefix("a/") == b.strip_prefix("b/") {
            return Some((a.to_string(), b.to_string()));
        }
    }
    if let Some(&pos) = positions.last() {
        return Some((rest[..pos].to_string(), rest[pos + 1..].to_string()));
    }
    // --no-prefix: "P P"
    let bytes = rest.len();
    if bytes % 2 == 1 {
        let half = bytes / 2;
        if rest.is_char_boundary(half)
            && rest.as_bytes()[half] == b' '
            && rest[..half] == rest[half + 1..]
        {
            return Some((rest[..half].to_string(), rest[half + 1..].to_string()));
        }
    }
    rest.split_once(' ')
        .map(|(a, b)| (a.to_string(), b.to_string()))
}

/// Path on a `---`/`+++` line: quoted or up to a tab (timestamps / trailing-space marker).
fn parse_marker_path(rest: &str, prefix: &str) -> Option<String> {
    let raw = if rest.starts_with('"') {
        take_quoted(rest)
            .map(|(p, _)| p)
            .unwrap_or_else(|| rest.to_string())
    } else {
        rest.split('\t')
            .next()
            .unwrap_or(rest)
            .trim_end()
            .to_string()
    };
    if raw == "/dev/null" {
        None
    } else {
        Some(strip_prefix_ab(&raw, prefix))
    }
}

fn dev_null_or(p: &str, prefix: &str) -> Option<String> {
    let p = unquote_path(p.trim());
    if p == "/dev/null" {
        None
    } else {
        Some(strip_prefix_ab(&p, prefix))
    }
}

fn strip_prefix_ab(p: &str, prefix: &str) -> String {
    p.strip_prefix(prefix).unwrap_or(p).to_string()
}

fn unquote_path(p: &str) -> String {
    let p = p.trim_end();
    if p.starts_with('"') {
        take_quoted(p)
            .map(|(s, _)| s)
            .unwrap_or_else(|| p.to_string())
    } else {
        p.to_string()
    }
}

/// Parses a git C-quoted string at the start of `s`; returns (unescaped, remainder).
fn take_quoted(s: &str) -> Option<(String, &str)> {
    let bytes = s.as_bytes();
    if bytes.first() != Some(&b'"') {
        return None;
    }
    let mut out: Vec<u8> = Vec::new();
    let mut i = 1;
    while i < bytes.len() {
        match bytes[i] {
            b'"' => return Some((String::from_utf8_lossy(&out).into_owned(), &s[i + 1..])),
            b'\\' if i + 1 < bytes.len() => {
                let c = bytes[i + 1];
                match c {
                    b'0'..=b'7' => {
                        let mut v: u32 = 0;
                        let mut j = i + 1;
                        while j < bytes.len() && j < i + 4 && (b'0'..=b'7').contains(&bytes[j]) {
                            v = v * 8 + (bytes[j] - b'0') as u32;
                            j += 1;
                        }
                        out.push(v as u8);
                        i = j;
                        continue;
                    }
                    b'n' => out.push(b'\n'),
                    b't' => out.push(b'\t'),
                    b'r' => out.push(b'\r'),
                    b'a' => out.push(7),
                    b'b' => out.push(8),
                    b'f' => out.push(12),
                    b'v' => out.push(11),
                    other => out.push(other),
                }
                i += 2;
            }
            other => {
                out.push(other);
                i += 1;
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------------------------
// ChangedIndex
// ---------------------------------------------------------------------------------------------

/// Fast lookup of which lines a diff touches, keyed by new path.
#[derive(Debug, Clone, Default)]
pub struct ChangedIndex {
    /// new_path -> added line numbers (new side)
    pub added: HashMap<String, BTreeSet<u32>>,
    /// new_path -> added + context line numbers inside hunks (new side)
    pub hunk_lines: HashMap<String, BTreeSet<u32>>,
    /// new_path -> deleted line numbers (old side)
    pub deleted: HashMap<String, BTreeSet<u32>>,
    /// new_path -> old_path (for renames)
    pub old_paths: HashMap<String, String>,
    /// new paths in diff order
    pub order: Vec<String>,
    /// new_path -> (new line -> old line) for context lines.
    pub context_new_to_old: HashMap<String, BTreeMap<u32, u32>>,
    /// new_path -> (old line -> new line) for context lines.
    pub context_old_to_new: HashMap<String, BTreeMap<u32, u32>>,
    /// Paths of files deleted by the diff (keyed like everything else, by their path).
    pub deleted_files: BTreeSet<String>,
    /// old_path -> new_path for renamed files, so lookups by the old name resolve.
    pub aliases: HashMap<String, String>,
}

impl ChangedIndex {
    pub fn from_files(files: &[FileDiff]) -> Self {
        let mut idx = ChangedIndex::default();
        for f in files {
            let key = f.new_path.clone();
            if !idx.old_paths.contains_key(&key) {
                idx.order.push(key.clone());
            }
            idx.old_paths.insert(key.clone(), f.old_path.clone());
            if f.old_path != f.new_path {
                idx.aliases.insert(f.old_path.clone(), key.clone());
            }
            if f.is_deleted {
                idx.deleted_files.insert(key.clone());
            }
            let added = idx.added.entry(key.clone()).or_default();
            let mut hunk_lines = BTreeSet::new();
            let mut deleted = BTreeSet::new();
            let mut n2o = BTreeMap::new();
            let mut o2n = BTreeMap::new();
            for l in f.hunks.iter().flat_map(|h| h.lines.iter()) {
                match l.kind {
                    LineKind::Add => {
                        if let Some(n) = l.new_line {
                            added.insert(n);
                            hunk_lines.insert(n);
                        }
                    }
                    LineKind::Del => {
                        if let Some(o) = l.old_line {
                            deleted.insert(o);
                        }
                    }
                    LineKind::Context => {
                        if let (Some(o), Some(n)) = (l.old_line, l.new_line) {
                            hunk_lines.insert(n);
                            n2o.insert(n, o);
                            o2n.insert(o, n);
                        }
                    }
                }
            }
            idx.hunk_lines
                .entry(key.clone())
                .or_default()
                .extend(hunk_lines);
            idx.deleted.entry(key.clone()).or_default().extend(deleted);
            idx.context_new_to_old
                .entry(key.clone())
                .or_default()
                .extend(n2o);
            idx.context_old_to_new.entry(key).or_default().extend(o2n);
        }
        idx
    }

    /// Maps a user/Claude-supplied path to the index key: exact match, then without `./` or a
    /// leading `/`, then without a `a/` / `b/` prefix, then a renamed file's old path.
    pub fn resolve(&self, path: &str) -> Option<&str> {
        let lookup = |p: &str| -> Option<&str> {
            if let Some((k, _)) = self.old_paths.get_key_value(p) {
                return Some(k.as_str());
            }
            self.aliases.get(p).map(|s| s.as_str())
        };
        if let Some(k) = lookup(path) {
            return Some(k);
        }
        let mut p = path.trim();
        while let Some(rest) = p.strip_prefix("./") {
            p = rest;
        }
        let p = p.trim_start_matches('/');
        if let Some(k) = lookup(p) {
            return Some(k);
        }
        for prefix in ["a/", "b/"] {
            if let Some(rest) = p.strip_prefix(prefix) {
                if let Some(k) = lookup(rest) {
                    return Some(k);
                }
            }
        }
        None
    }

    /// True if `path` (or a renamed file's old path) is part of the diff.
    pub fn contains(&self, path: &str) -> bool {
        self.resolve(path).is_some()
    }

    /// True if `new_line` is an ADDED line of `path`.
    pub fn is_changed(&self, path: &str, new_line: u32) -> bool {
        self.resolve(path)
            .and_then(|k| self.added.get(k))
            .is_some_and(|s| s.contains(&new_line))
    }

    /// True if `old_line` is a DELETED line of `path` (old side).
    pub fn is_deleted_line(&self, path: &str, old_line: u32) -> bool {
        self.resolve(path)
            .and_then(|k| self.deleted.get(k))
            .is_some_and(|s| s.contains(&old_line))
    }

    /// True if `new_line` is an added or context line inside a hunk of `path`.
    pub fn in_hunk(&self, path: &str, new_line: u32) -> bool {
        self.resolve(path)
            .and_then(|k| self.hunk_lines.get(k))
            .is_some_and(|s| s.contains(&new_line))
    }

    /// GitLab-style position for a line note. `side` is `"new"` or `"old"`; `shas` = (base, start, head).
    ///
    /// - added line (side new)   => `new_line` only
    /// - deleted line (side old) => `old_line` only
    /// - context line (either side, inside a hunk) => both
    /// - anything else => `None` (callers fall back to a file-level note)
    pub fn position(
        &self,
        path: &str,
        line: u32,
        side: &str,
        shas: (&str, &str, &str),
    ) -> Option<DiffPosition> {
        let key = self.resolve(path)?;
        let (old_line, new_line) = match side.trim().to_ascii_lowercase().as_str() {
            "new" | "right" | "" => {
                if self.added.get(key).is_some_and(|s| s.contains(&line)) {
                    (None, Some(line))
                } else {
                    let old = *self.context_new_to_old.get(key)?.get(&line)?;
                    (Some(old), Some(line))
                }
            }
            "old" | "left" => {
                if self.deleted.get(key).is_some_and(|s| s.contains(&line)) {
                    (Some(line), None)
                } else {
                    let new = *self.context_old_to_new.get(key)?.get(&line)?;
                    (Some(line), Some(new))
                }
            }
            _ => return None,
        };
        let old_path = self
            .old_paths
            .get(key)
            .cloned()
            .unwrap_or_else(|| key.to_string());
        Some(DiffPosition {
            base_sha: shas.0.to_string(),
            start_sha: shas.1.to_string(),
            head_sha: shas.2.to_string(),
            old_path,
            new_path: key.to_string(),
            old_line,
            new_line,
        })
    }

    /// New paths in diff order, excluding files the diff deletes (they no longer exist at head, so
    /// linters given `{changed_files}` must not see them).
    pub fn changed_files(&self) -> Vec<String> {
        self.order
            .iter()
            .filter(|p| !self.deleted_files.contains(*p))
            .cloned()
            .collect()
    }
}

// ---------------------------------------------------------------------------------------------
// Glob helpers
// ---------------------------------------------------------------------------------------------

/// Compiled gitignore-flavoured glob list: `*` does not cross `/`, `**` does, and a pattern without
/// a `/` (e.g. `yarn.lock`, `*.py`) also matches the file name at any depth.
#[derive(Debug, Clone)]
pub struct PathMatcher {
    full: GlobSet,
    basename: GlobSet,
}

impl PathMatcher {
    /// Builds a matcher; invalid globs are an error.
    pub fn new<S: AsRef<str>>(globs: &[S]) -> Result<Self, globset::Error> {
        let mut full = GlobSetBuilder::new();
        let mut basename = GlobSetBuilder::new();
        for g in globs {
            let g = g.as_ref().trim();
            if g.is_empty() {
                continue;
            }
            let g = g.trim_start_matches("./");
            let g = g.strip_prefix('/').unwrap_or(g);
            // "dir/" means everything below dir.
            let owned;
            let g = if let Some(stripped) = g.strip_suffix('/') {
                owned = format!("{stripped}/**");
                owned.as_str()
            } else {
                g
            };
            let glob = GlobBuilder::new(g).literal_separator(true).build()?;
            full.add(glob.clone());
            if !g.contains('/') {
                basename.add(glob);
            }
        }
        Ok(Self {
            full: full.build()?,
            basename: basename.build()?,
        })
    }

    /// Builds a matcher, skipping (and logging) invalid globs.
    pub fn lenient<S: AsRef<str>>(globs: &[S]) -> Self {
        let valid: Vec<&str> = globs
            .iter()
            .map(|g| g.as_ref())
            .filter(|g| match GlobBuilder::new(g.trim()).build() {
                Ok(_) => true,
                Err(e) => {
                    tracing::warn!("ignoring invalid glob {g:?}: {e}");
                    false
                }
            })
            .collect();
        Self::new(&valid).unwrap_or_else(|_| Self::empty())
    }

    pub fn empty() -> Self {
        Self {
            full: GlobSet::empty(),
            basename: GlobSet::empty(),
        }
    }

    pub fn is_match(&self, path: &str) -> bool {
        let path = path.trim_start_matches("./");
        if self.full.is_match(path) {
            return true;
        }
        let name = path.rsplit('/').next().unwrap_or(path);
        self.basename.is_match(name)
    }
}

/// Drops files whose new (or old, for deletions) path matches any glob.
pub fn filter_ignored(files: Vec<FileDiff>, ignore_globs: &[String]) -> Vec<FileDiff> {
    if ignore_globs.iter().all(|g| g.trim().is_empty()) {
        return files;
    }
    let m = PathMatcher::lenient(ignore_globs);
    files
        .into_iter()
        .filter(|f| {
            let path = if f.is_deleted {
                &f.old_path
            } else {
                &f.new_path
            };
            !m.is_match(path)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hunk_header_without_counts() {
        let d = "--- a/x\n+++ b/x\n@@ -1 +1 @@\n-old\n+new\n";
        let f = parse_unified(d);
        assert_eq!(f.len(), 1);
        let h = &f[0].hunks[0];
        assert_eq!(
            (h.old_start, h.old_lines, h.new_start, h.new_lines),
            (1, 1, 1, 1)
        );
        assert_eq!(h.lines.len(), 2);
    }

    #[test]
    fn quoted_paths() {
        let (p, rest) = take_quoted("\"a/caf\\303\\251 x.py\" tail").unwrap();
        assert_eq!(p, "a/café x.py");
        assert_eq!(rest, " tail");
        assert_eq!(
            split_git_header_paths("a/dir b/x.py b/dir b/x.py"),
            Some(("a/dir b/x.py".into(), "b/dir b/x.py".into()))
        );
    }

    #[test]
    fn matcher_semantics() {
        let m = PathMatcher::new(&["yarn.lock", "**/*.min.js", "**/dist/**", "docs/"]).unwrap();
        assert!(m.is_match("yarn.lock"));
        assert!(m.is_match("ui/yarn.lock"));
        assert!(m.is_match("a.min.js"));
        assert!(m.is_match("x/y/a.min.js"));
        assert!(m.is_match("ui/dist/app.js"));
        assert!(m.is_match("docs/readme.md"));
        assert!(!m.is_match("src/app.js"));
        let py = PathMatcher::new(&["*.py"]).unwrap();
        assert!(py.is_match("a/b/c.py"));
        let scoped = PathMatcher::new(&["src/*.py"]).unwrap();
        assert!(scoped.is_match("src/a.py"));
        assert!(!scoped.is_match("src/sub/a.py"));
    }
}
