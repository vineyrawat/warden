//! diff.rs: unified diff parsing, ChangedIndex, filter_ignored.

#[path = "analysis_fixtures.rs"]
mod fixtures;

use fixtures::*;
use pretty_assertions::assert_eq;
use warden_core::diff::{
    filter_ignored, parse_unified, ChangedIndex, DiffLine, FileDiff, LineKind, PathMatcher,
};
use warden_core::types::DiffPosition;

fn file<'a>(files: &'a [FileDiff], path: &str) -> &'a FileDiff {
    files
        .iter()
        .find(|f| f.new_path == path)
        .unwrap_or_else(|| {
            panic!(
                "no file {path} in {:?}",
                files.iter().map(|f| &f.new_path).collect::<Vec<_>>()
            )
        })
}

#[test]
fn parses_every_file_in_order() {
    let files = parse_unified(MR_DIFF);
    let paths: Vec<&str> = files.iter().map(|f| f.new_path.as_str()).collect();
    assert_eq!(
        paths,
        vec![
            IMPORT_LINKS,
            TEST_FILE,
            LEGACY,
            NEW_NAME,
            "docs/b.md",
            "run.sh",
            "ui/public/logo.png",
            "ui/public/new.png",
            "yarn.lock",
            TSX,
        ]
    );
}

#[test]
fn modified_file_hunks_and_line_numbers() {
    let files = parse_unified(MR_DIFF);
    let f = file(&files, IMPORT_LINKS);
    assert_eq!(f.old_path, IMPORT_LINKS);
    assert!(!f.is_new && !f.is_deleted && !f.is_binary);
    assert_eq!(f.hunks.len(), 2);

    let h = &f.hunks[0];
    assert_eq!(
        (h.old_start, h.old_lines, h.new_start, h.new_lines),
        (8, 7, 8, 14)
    );
    assert_eq!(h.header, "@@ -8,7 +8,14 @@ import json");
    assert_eq!(h.lines.len(), 15);
    // first context line (blank) keeps both numbers
    assert_eq!(
        h.lines[0],
        DiffLine {
            kind: LineKind::Context,
            old_line: Some(8),
            new_line: Some(8),
            text: String::new()
        }
    );
    // the deleted comment
    assert_eq!(
        h.lines[3],
        DiffLine {
            kind: LineKind::Del,
            old_line: Some(11),
            new_line: None,
            text: "# legacy linker removed".into()
        }
    );
    // the f-string SQL is new line 16, the commit 17 (as in the docs/plan.html mockup)
    let sql = h
        .lines
        .iter()
        .find(|l| l.text.contains("frappe.db.sql"))
        .unwrap();
    assert_eq!(
        (sql.kind, sql.old_line, sql.new_line),
        (LineKind::Add, None, Some(16))
    );
    assert!(sql
        .text
        .starts_with("        frappe.db.sql(f\"update `tabECRM Lead`"));
    let commit = h.lines.iter().find(|l| l.text.contains("commit")).unwrap();
    assert_eq!(commit.new_line, Some(17));
    let whitelist = h
        .lines
        .iter()
        .find(|l| l.text == "@frappe.whitelist()")
        .unwrap();
    assert_eq!(whitelist.new_line, Some(12));
    // trailing context after the additions
    let last = h.lines.last().unwrap();
    assert_eq!(
        (last.kind, last.old_line, last.new_line),
        (LineKind::Context, Some(14), Some(21))
    );

    let h2 = &f.hunks[1];
    assert_eq!(
        (h2.old_start, h2.old_lines, h2.new_start, h2.new_lines),
        (40, 3, 47, 4)
    );
    let nums: Vec<(LineKind, Option<u32>, Option<u32>)> = h2
        .lines
        .iter()
        .map(|l| (l.kind, l.old_line, l.new_line))
        .collect();
    assert_eq!(
        nums,
        vec![
            (LineKind::Context, Some(40), Some(47)),
            (LineKind::Context, Some(41), Some(48)),
            (LineKind::Del, Some(42), None),
            (LineKind::Add, None, Some(49)),
            (LineKind::Add, None, Some(50)),
        ]
    );
    assert_eq!(f.stats(), (10, 2));
}

#[test]
fn new_file_from_dev_null() {
    let files = parse_unified(MR_DIFF);
    let f = file(&files, TEST_FILE);
    assert!(f.is_new);
    assert!(!f.is_deleted);
    assert_eq!(
        f.old_path, TEST_FILE,
        "new files use the new path on both sides"
    );
    assert_eq!(f.hunks.len(), 1);
    assert_eq!((f.hunks[0].old_start, f.hunks[0].old_lines), (0, 0));
    let added: Vec<u32> = f.added_lines().map(|(n, _)| n).collect();
    assert_eq!(added, vec![1, 2, 3, 4]);
}

#[test]
fn deleted_file_to_dev_null() {
    let files = parse_unified(MR_DIFF);
    let f = file(&files, LEGACY);
    assert!(f.is_deleted);
    assert!(!f.is_new);
    assert_eq!(f.old_path, LEGACY);
    let dels: Vec<Option<u32>> = f.hunks[0].lines.iter().map(|l| l.old_line).collect();
    assert_eq!(dels, vec![Some(1), Some(2), Some(3)]);
    assert!(f.hunks[0]
        .lines
        .iter()
        .all(|l| l.kind == LineKind::Del && l.new_line.is_none()));
    assert_eq!(f.added_lines().count(), 0);
}

#[test]
fn rename_with_edit_and_no_newline_marker() {
    let files = parse_unified(MR_DIFF);
    let f = file(&files, NEW_NAME);
    assert_eq!(f.old_path, OLD_NAME);
    assert!(f.is_rename());
    assert_eq!(f.hunks.len(), 1);
    let texts: Vec<&str> = f.hunks[0].lines.iter().map(|l| l.text.as_str()).collect();
    assert_eq!(
        texts,
        vec!["def helper():", "    return 1", "    return 2", "# end"]
    );
    assert!(
        !texts.iter().any(|t| t.contains("No newline")),
        "the \\ marker is not a diff line"
    );
}

#[test]
fn pure_rename_mode_change_and_binaries() {
    let files = parse_unified(MR_DIFF);
    let r = file(&files, "docs/b.md");
    assert_eq!(r.old_path, "docs/a.md");
    assert!(r.hunks.is_empty());
    assert!(r.is_rename());

    let m = file(&files, "run.sh");
    assert_eq!(m.old_path, "run.sh");
    assert!(m.hunks.is_empty() && !m.is_binary && !m.is_new);

    let logo = file(&files, "ui/public/logo.png");
    assert!(logo.is_binary && !logo.is_new && !logo.is_deleted);
    assert!(logo.hunks.is_empty());

    let new_png = file(&files, "ui/public/new.png");
    assert!(new_png.is_binary && new_png.is_new);
}

#[test]
fn binary_without_git_header_and_deleted_binary() {
    let d = "Binary files a/img/x.png and /dev/null differ\n";
    // No `diff --git` header and no section: nothing to attach it to -> ignored gracefully.
    assert!(parse_unified(d).is_empty());

    let d = "diff --git a/img/x.png b/img/x.png\ndeleted file mode 100644\nindex 1..0\nBinary files a/img/x.png and /dev/null differ\n";
    let files = parse_unified(d);
    assert_eq!(files.len(), 1);
    assert!(files[0].is_binary && files[0].is_deleted);
    assert_eq!(files[0].new_path, "img/x.png");
}

#[test]
fn git_binary_patch_payload_is_ignored() {
    let d = "\
diff --git a/a.bin b/a.bin
index 1234567..89abcde 100644
GIT binary patch
literal 12
TcmZ?wbhEHbWMp7q;Nk!Q2LJ#7

literal 0
HcmV?d00001

diff --git a/b.py b/b.py
--- a/b.py
+++ b/b.py
@@ -1 +1 @@
-x = 1
+x = 2
";
    let files = parse_unified(d);
    assert_eq!(files.len(), 2);
    assert!(files[0].is_binary);
    assert!(files[0].hunks.is_empty());
    assert_eq!(files[1].new_path, "b.py");
    assert_eq!(
        files[1].added_lines().collect::<Vec<_>>(),
        vec![(1, "x = 2")]
    );
}

#[test]
fn hunk_headers_without_counts() {
    let d = "\
diff --git a/a.txt b/a.txt
--- a/a.txt
+++ b/a.txt
@@ -5 +5,2 @@ section text
 keep
+added
@@ -20,2 +21 @@
-gone
 stay
";
    let files = parse_unified(d);
    let h = &files[0].hunks;
    assert_eq!(h.len(), 2);
    assert_eq!(
        (
            h[0].old_start,
            h[0].old_lines,
            h[0].new_start,
            h[0].new_lines
        ),
        (5, 1, 5, 2)
    );
    assert_eq!(h[0].header, "@@ -5 +5,2 @@ section text");
    assert_eq!(
        (
            h[1].old_start,
            h[1].old_lines,
            h[1].new_start,
            h[1].new_lines
        ),
        (20, 2, 21, 1)
    );
    assert_eq!(h[1].lines[0].old_line, Some(20));
    assert_eq!(h[1].lines[1].new_line, Some(21));
}

#[test]
fn content_that_looks_like_headers_inside_a_hunk() {
    // A deleted SQL comment "-- old" renders as "--- old"; an added "++ x" as "+++ x".
    let d = "\
diff --git a/q.sql b/q.sql
--- a/q.sql
+++ b/q.sql
@@ -1,3 +1,3 @@
--- old comment
+++ new comment
 select 1;
-@@ not a header
+diff --git lookalike
";
    let files = parse_unified(d);
    assert_eq!(files.len(), 1, "{files:#?}");
    let lines = &files[0].hunks[0].lines;
    assert_eq!(lines.len(), 5);
    assert_eq!(
        (lines[0].kind, lines[0].text.as_str()),
        (LineKind::Del, "-- old comment")
    );
    assert_eq!(
        (lines[1].kind, lines[1].text.as_str()),
        (LineKind::Add, "++ new comment")
    );
    assert_eq!(
        (lines[3].kind, lines[3].text.as_str()),
        (LineKind::Del, "@@ not a header")
    );
    assert_eq!(
        (lines[4].kind, lines[4].text.as_str()),
        (LineKind::Add, "diff --git lookalike")
    );
}

#[test]
fn plain_unified_diff_without_git_headers() {
    let d = "\
--- old/file.txt\t2026-01-01 10:00:00.000000000 +0000
+++ new/file.txt\t2026-01-02 10:00:00.000000000 +0000
@@ -1,2 +1,2 @@
-a
+b
 c
--- old/other.txt\t2026-01-01 10:00:00.000000000 +0000
+++ new/other.txt\t2026-01-02 10:00:00.000000000 +0000
@@ -1 +1,2 @@
 z
+y
";
    let files = parse_unified(d);
    assert_eq!(files.len(), 2);
    assert_eq!(files[0].old_path, "old/file.txt");
    assert_eq!(files[0].new_path, "new/file.txt");
    assert_eq!(files[1].new_path, "new/other.txt");
    assert_eq!(files[1].added_lines().collect::<Vec<_>>(), vec![(2, "y")]);
}

#[test]
fn quoted_paths_with_spaces_and_unicode() {
    let d = "\
diff --git \"a/sp ace/caf\\303\\251.py\" \"b/sp ace/caf\\303\\251.py\"
index 1..2 100644
--- \"a/sp ace/caf\\303\\251.py\"
+++ \"b/sp ace/caf\\303\\251.py\"
@@ -1 +1 @@
-a
+b
diff --git a/with space.txt b/with space.txt
new file mode 100644
--- /dev/null
+++ b/with space.txt\t
@@ -0,0 +1 @@
+hello
";
    let files = parse_unified(d);
    assert_eq!(files.len(), 2);
    assert_eq!(files[0].new_path, "sp ace/café.py");
    assert_eq!(files[0].old_path, "sp ace/café.py");
    assert_eq!(files[1].new_path, "with space.txt");
    assert!(files[1].is_new);
}

#[test]
fn pure_rename_with_quoted_rename_lines() {
    let d = "diff --git \"a/x y.md\" \"b/z w.md\"\nsimilarity index 100%\nrename from \"x y.md\"\nrename to \"z w.md\"\n";
    let files = parse_unified(d);
    assert_eq!(files[0].old_path, "x y.md");
    assert_eq!(files[0].new_path, "z w.md");
}

#[test]
fn crlf_and_whitespace_stripped_blank_context_lines() {
    // Same hunk as git would print, but CRLF line endings and the blank context line's leading
    // space removed by an editor.
    let d = "diff --git a/a.py b/a.py\r\n--- a/a.py\r\n+++ b/a.py\r\n@@ -1,3 +1,4 @@\r\n import x\r\n\r\n+y = 1\r\n z = 2\r\n";
    let files = parse_unified(d);
    let lines = &files[0].hunks[0].lines;
    assert_eq!(lines.len(), 4);
    assert_eq!(
        (lines[1].kind, lines[1].old_line, lines[1].new_line),
        (LineKind::Context, Some(2), Some(2))
    );
    assert_eq!(
        (lines[2].kind, lines[2].new_line, lines[2].text.as_str()),
        (LineKind::Add, Some(3), "y = 1")
    );
    assert_eq!(lines[3].new_line, Some(4));
}

#[test]
fn truncated_hunk_does_not_panic() {
    let d = "diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1,10 +1,10 @@\n a\n-b\n+c\n";
    let files = parse_unified(d);
    assert_eq!(files[0].hunks[0].lines.len(), 3);
    assert!(parse_unified("").is_empty());
    assert!(parse_unified("garbage\nmore garbage\n").is_empty());
}

#[test]
fn copy_is_treated_like_rename() {
    let d = "diff --git a/a.py b/b.py\nsimilarity index 100%\ncopy from a.py\ncopy to b.py\n";
    let f = &parse_unified(d)[0];
    assert_eq!((f.old_path.as_str(), f.new_path.as_str()), ("a.py", "b.py"));
}

// ---------------------------------------------------------------------------------------------
// ChangedIndex
// ---------------------------------------------------------------------------------------------

fn index() -> ChangedIndex {
    ChangedIndex::from_files(&parse_unified(MR_DIFF))
}

#[test]
fn is_changed_only_for_added_lines() {
    let idx = index();
    for n in 11..=18 {
        assert!(idx.is_changed(IMPORT_LINKS, n), "line {n}");
    }
    assert!(idx.is_changed(IMPORT_LINKS, 49));
    assert!(idx.is_changed(IMPORT_LINKS, 50));
    assert!(!idx.is_changed(IMPORT_LINKS, 10), "context");
    assert!(!idx.is_changed(IMPORT_LINKS, 19), "context");
    assert!(!idx.is_changed(IMPORT_LINKS, 30), "outside hunks");
    assert!(!idx.is_changed("nope.py", 1));
    assert!(!idx.is_changed(LEGACY, 1), "deleted file has no new lines");
}

#[test]
fn in_hunk_includes_context() {
    let idx = index();
    assert!(idx.in_hunk(IMPORT_LINKS, 8));
    assert!(idx.in_hunk(IMPORT_LINKS, 16));
    assert!(idx.in_hunk(IMPORT_LINKS, 21));
    assert!(!idx.in_hunk(IMPORT_LINKS, 22));
    assert!(idx.in_hunk(IMPORT_LINKS, 47));
    assert!(!idx.in_hunk(IMPORT_LINKS, 30));
}

#[test]
fn deleted_lines_are_indexed_on_the_old_side() {
    let idx = index();
    assert!(idx.is_deleted_line(IMPORT_LINKS, 11));
    assert!(idx.is_deleted_line(IMPORT_LINKS, 42));
    assert!(!idx.is_deleted_line(IMPORT_LINKS, 12));
    assert!(idx.is_deleted_line(LEGACY, 2));
}

fn pos(old_path: &str, new_path: &str, old: Option<u32>, new: Option<u32>) -> DiffPosition {
    DiffPosition {
        base_sha: SHAS.0.into(),
        start_sha: SHAS.1.into(),
        head_sha: SHAS.2.into(),
        old_path: old_path.into(),
        new_path: new_path.into(),
        old_line: old,
        new_line: new,
    }
}

#[test]
fn position_added_deleted_and_context_lines() {
    let idx = index();
    // added => new_line only
    assert_eq!(
        idx.position(IMPORT_LINKS, 16, "new", SHAS),
        Some(pos(IMPORT_LINKS, IMPORT_LINKS, None, Some(16)))
    );
    // deleted => old_line only
    assert_eq!(
        idx.position(IMPORT_LINKS, 11, "old", SHAS),
        Some(pos(IMPORT_LINKS, IMPORT_LINKS, Some(11), None))
    );
    // context (new side) => both
    assert_eq!(
        idx.position(IMPORT_LINKS, 19, "new", SHAS),
        Some(pos(IMPORT_LINKS, IMPORT_LINKS, Some(12), Some(19)))
    );
    assert_eq!(
        idx.position(IMPORT_LINKS, 48, "new", SHAS),
        Some(pos(IMPORT_LINKS, IMPORT_LINKS, Some(41), Some(48)))
    );
    // context (old side) => both
    assert_eq!(
        idx.position(IMPORT_LINKS, 12, "old", SHAS),
        Some(pos(IMPORT_LINKS, IMPORT_LINKS, Some(12), Some(19)))
    );
    // outside any hunk / unknown file / bad side
    assert_eq!(idx.position(IMPORT_LINKS, 30, "new", SHAS), None);
    assert_eq!(
        idx.position(IMPORT_LINKS, 16, "old", SHAS),
        None,
        "old line 16 is not in a hunk"
    );
    assert_eq!(idx.position("missing.py", 1, "new", SHAS), None);
    assert_eq!(idx.position(IMPORT_LINKS, 16, "sideways", SHAS), None);
    // case-insensitive side
    assert!(idx.position(IMPORT_LINKS, 16, "NEW", SHAS).is_some());
}

#[test]
fn position_for_renamed_new_and_deleted_files() {
    let idx = index();
    assert_eq!(
        idx.position(NEW_NAME, 2, "new", SHAS),
        Some(pos(OLD_NAME, NEW_NAME, None, Some(2)))
    );
    assert_eq!(
        idx.position(NEW_NAME, 2, "old", SHAS),
        Some(pos(OLD_NAME, NEW_NAME, Some(2), None))
    );
    // looking up by the renamed file's old path resolves to the same file
    assert_eq!(
        idx.position(OLD_NAME, 2, "old", SHAS),
        Some(pos(OLD_NAME, NEW_NAME, Some(2), None))
    );
    assert!(idx.is_deleted_line(OLD_NAME, 2));
    // new file
    assert_eq!(
        idx.position(TEST_FILE, 3, "new", SHAS),
        Some(pos(TEST_FILE, TEST_FILE, None, Some(3)))
    );
    // deleted file
    assert_eq!(
        idx.position(LEGACY, 2, "old", SHAS),
        Some(pos(LEGACY, LEGACY, Some(2), None))
    );
    assert_eq!(idx.position(LEGACY, 2, "new", SHAS), None);
}

#[test]
fn resolve_normalizes_paths() {
    let idx = index();
    assert_eq!(idx.resolve(IMPORT_LINKS), Some(IMPORT_LINKS));
    assert_eq!(
        idx.resolve(&format!("./{IMPORT_LINKS}")),
        Some(IMPORT_LINKS)
    );
    assert_eq!(idx.resolve(&format!("/{IMPORT_LINKS}")), Some(IMPORT_LINKS));
    assert_eq!(
        idx.resolve(&format!("b/{IMPORT_LINKS}")),
        Some(IMPORT_LINKS)
    );
    assert_eq!(idx.resolve(OLD_NAME), Some(NEW_NAME));
    assert_eq!(idx.resolve("entries_crm/api/missing.py"), None);
    assert!(idx.is_changed(&format!("./{IMPORT_LINKS}"), 16));
    assert!(idx.contains("docs/b.md"));
    assert!(idx.contains("docs/a.md"));
}

#[test]
fn changed_files_in_order_without_deleted() {
    let idx = index();
    assert_eq!(
        idx.changed_files(),
        vec![
            IMPORT_LINKS.to_string(),
            TEST_FILE.into(),
            NEW_NAME.into(),
            "docs/b.md".into(),
            "run.sh".into(),
            "ui/public/logo.png".into(),
            "ui/public/new.png".into(),
            "yarn.lock".into(),
            TSX.into(),
        ]
    );
    assert_eq!(
        idx.old_paths.get(NEW_NAME).map(String::as_str),
        Some(OLD_NAME)
    );
}

#[test]
fn empty_index() {
    let idx = ChangedIndex::from_files(&[]);
    assert!(idx.changed_files().is_empty());
    assert!(!idx.is_changed("a", 1));
    assert!(idx.position("a", 1, "new", SHAS).is_none());
}

// ---------------------------------------------------------------------------------------------
// filter_ignored
// ---------------------------------------------------------------------------------------------

#[test]
fn filter_ignored_with_default_globs() {
    let files = parse_unified(MR_DIFF);
    let globs: Vec<String> = vec![
        "yarn.lock".into(),
        "**/*.min.js".into(),
        "**/dist/**".into(),
    ];
    let kept = filter_ignored(files.clone(), &globs);
    assert_eq!(kept.len(), files.len() - 1);
    assert!(!kept.iter().any(|f| f.new_path == "yarn.lock"));

    let kept = filter_ignored(files.clone(), &["**/*.png".into(), "docs/".into()]);
    let paths: Vec<&str> = kept.iter().map(|f| f.new_path.as_str()).collect();
    assert!(!paths.contains(&"ui/public/logo.png"));
    assert!(!paths.contains(&"ui/public/new.png"));
    assert!(!paths.contains(&"docs/b.md"));
    assert!(paths.contains(&IMPORT_LINKS));
}

#[test]
fn filter_ignored_matches_old_path_of_deleted_files() {
    let files = parse_unified(MR_DIFF);
    let kept = filter_ignored(files, &["entries_crm/legacy/**".into()]);
    assert!(!kept.iter().any(|f| f.old_path == LEGACY));
}

#[test]
fn filter_ignored_edge_cases() {
    let files = parse_unified(MR_DIFF);
    let n = files.len();
    assert_eq!(filter_ignored(files.clone(), &[]).len(), n);
    assert_eq!(filter_ignored(files.clone(), &["  ".into()]).len(), n);
    // an invalid glob is skipped, the valid one still applies
    let kept = filter_ignored(files.clone(), &["[".into(), "*.tsx".into()]);
    assert_eq!(kept.len(), n - 1);
    // `*` does not cross directories for globs with a slash
    let kept = filter_ignored(files, &["entries_crm/*.py".into()]);
    assert_eq!(kept.len(), n);
}

#[test]
fn path_matcher_errors_on_invalid_glob() {
    assert!(PathMatcher::new(&["a/[b"]).is_err());
    assert!(PathMatcher::new(&["**/*.py"]).unwrap().is_match("x.py"));
}

#[test]
fn file_diff_serializes_for_the_ui() {
    let files = parse_unified(MR_DIFF);
    let v = serde_json::to_value(&files[0]).unwrap();
    assert_eq!(v["new_path"], IMPORT_LINKS);
    assert_eq!(v["hunks"][0]["lines"][3]["kind"], "del");
    let back: FileDiff = serde_json::from_value(v).unwrap();
    assert_eq!(back, files[0]);
}
