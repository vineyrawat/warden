//! WorktreeManager integration tests against throwaway local git repos:
//! a bare "remote" (with refs/merge-requests/<iid>/head and refs/pull/<n>/head) and a clone acting as
//! `project.local_path`. No network.

use std::path::{Path, PathBuf};
use std::process::Command;

use warden_core::worktree::{project_dir_name, WorktreeManager};
use warden_core::{ForgeKind, MergeRequest, PostingMode, Project};

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "user.name=Warden Test",
            "-c",
            "user.email=test@warden.invalid",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "tag.gpgsign=false",
            "-c",
            "protocol.file.allow=always",
        ])
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .expect("git runs");
    assert!(
        out.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

struct Fixture {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    remote: PathBuf,
    seed: PathBuf,
    clone: PathBuf,
    base_sha: String,
    head_sha: String,
}

/// remote.git (bare) <- seed (pushes main + feature) ; clone = project.local_path
fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().canonicalize().unwrap();
    let remote = base.join("remote.git");
    let seed = base.join("seed");
    let clone = base.join("clone");
    let root = base.join("worktrees");

    std::fs::create_dir_all(&remote).unwrap();
    git(&remote, &["init", "--bare", "-q", "-b", "main"]);
    std::fs::create_dir_all(&seed).unwrap();
    git(&seed, &["init", "-q", "-b", "main"]);
    std::fs::write(seed.join("app.py"), "def hello():\n    return 1\n").unwrap();
    std::fs::write(seed.join("README.md"), "# demo\n").unwrap();
    git(&seed, &["add", "."]);
    git(&seed, &["commit", "-q", "-m", "init"]);
    git(
        &seed,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    git(&seed, &["push", "-q", "origin", "main"]);
    let base_sha = git(&seed, &["rev-parse", "HEAD"]);

    git(&seed, &["checkout", "-q", "-b", "feature"]);
    std::fs::write(
        seed.join("app.py"),
        "def hello():\n    return 2\n\n\ndef added():\n    return 'new line'\n",
    )
    .unwrap();
    git(&seed, &["commit", "-q", "-am", "feature work"]);
    git(&seed, &["push", "-q", "origin", "feature"]);
    let head_sha = git(&seed, &["rev-parse", "HEAD"]);
    git(
        &remote,
        &["update-ref", "refs/merge-requests/1/head", &head_sha],
    );
    git(&remote, &["update-ref", "refs/pull/2/head", &head_sha]);

    // Clone before the MR exists locally: only main is checked out.
    git(
        &base,
        &[
            "clone",
            "-q",
            remote.to_str().unwrap(),
            clone.to_str().unwrap(),
        ],
    );

    Fixture {
        _tmp: tmp,
        root,
        remote,
        seed,
        clone,
        base_sha,
        head_sha,
    }
}

fn project(f: &Fixture, forge: ForgeKind) -> Project {
    Project {
        id: 1,
        forge,
        base_url: "https://gitlab.example.com".into(),
        remote_id: "entries/demo".into(),
        name: "entries/demo app".into(),
        local_path: f.clone.to_string_lossy().into_owned(),
        target_branches: vec!["main".into()],
        mode: PostingMode::Draft,
        poll_interval_secs: 60,
        enabled: true,
    }
}

fn mr(f: &Fixture, iid: u64, head: &str) -> MergeRequest {
    MergeRequest {
        id: 10,
        project_id: 1,
        iid,
        title: "Feature".into(),
        source_branch: "feature".into(),
        target_branch: "main".into(),
        head_sha: head.into(),
        base_sha: f.base_sha.clone(),
        start_sha: f.base_sha.clone(),
        state: "opened".into(),
        ..Default::default()
    }
}

#[tokio::test]
async fn prepare_creates_detached_worktree_at_head() {
    let f = fixture();
    let mgr = WorktreeManager::new(&f.root);
    let p = project(&f, ForgeKind::Gitlab);
    let wt = mgr.prepare(&p, &mr(&f, 1, &f.head_sha)).await.unwrap();

    // `<root>/<name>-<hash(base_url, remote_id)>/<iid>` so same-named projects never collide.
    assert_eq!(wt, f.root.join(project_dir_name(&p)).join("1"));
    assert!(project_dir_name(&p).starts_with("entries_demo_app-"));
    assert_eq!(git(&wt, &["rev-parse", "HEAD"]), f.head_sha);
    // detached
    let sym = Command::new("git")
        .arg("-C")
        .arg(&wt)
        .args(["symbolic-ref", "-q", "HEAD"])
        .status()
        .unwrap();
    assert!(!sym.success(), "worktree HEAD should be detached");
    assert!(std::fs::read_to_string(wt.join("app.py"))
        .unwrap()
        .contains("new line"));
    // the MR ref was fetched into the project repo
    assert_eq!(
        git(&f.clone, &["rev-parse", "refs/warden/mr/1"]),
        f.head_sha
    );
    // registered as a worktree of the clone
    assert!(git(&f.clone, &["worktree", "list", "--porcelain"]).contains(wt.to_str().unwrap()));
}

#[tokio::test]
async fn diff_three_dot_and_two_dot_fallback() {
    let f = fixture();
    let mgr = WorktreeManager::new(&f.root);
    let p = project(&f, ForgeKind::Gitlab);
    let wt = mgr.prepare(&p, &mr(&f, 1, &f.head_sha)).await.unwrap();

    let d = mgr.diff(&wt, &f.base_sha, &f.head_sha).await.unwrap();
    assert!(d.contains("diff --git a/app.py b/app.py"), "{d}");
    assert!(d.contains("+    return 'new line'"), "{d}");
    assert!(d.contains("-    return 1"), "{d}");
    assert!(!d.contains("\u{1b}["), "no colour codes");
    assert!(!d.contains("README"), "unchanged files are not in the diff");

    // An orphan commit has no merge base with head: base...head fails, base..head is used.
    let tree = git(
        &f.clone,
        &["rev-parse", &format!("{}^{{tree}}", f.base_sha)],
    );
    let orphan = git(&f.clone, &["commit-tree", &tree, "-m", "orphan"]);
    let d2 = mgr.diff(&wt, &orphan, &f.head_sha).await.unwrap();
    assert!(d2.contains("+    return 'new line'"), "{d2}");

    // Garbage refs are a Git error.
    let err = mgr.diff(&wt, "nope", "alsonope").await.unwrap_err();
    assert!(matches!(err, warden_core::WardenError::Git(_)), "{err}");
}

#[tokio::test]
async fn prepare_updates_existing_worktree_to_new_head() {
    let f = fixture();
    let mgr = WorktreeManager::new(&f.root);
    let p = project(&f, ForgeKind::Gitlab);
    let wt = mgr.prepare(&p, &mr(&f, 1, &f.head_sha)).await.unwrap();
    // leftovers from a previous run: an untracked file and a modified tracked file
    std::fs::write(wt.join("scratch.txt"), "junk").unwrap();
    std::fs::write(wt.join("README.md"), "locally modified\n").unwrap();

    // new push to the MR (force-push style: amend)
    std::fs::write(f.seed.join("app.py"), "def hello():\n    return 3\n").unwrap();
    git(&f.seed, &["commit", "-q", "--amend", "-am", "feature v2"]);
    git(&f.seed, &["push", "-q", "-f", "origin", "feature"]);
    let new_head = git(&f.seed, &["rev-parse", "HEAD"]);
    git(
        &f.remote,
        &["update-ref", "refs/merge-requests/1/head", &new_head],
    );

    let wt2 = mgr.prepare(&p, &mr(&f, 1, &new_head)).await.unwrap();
    assert_eq!(wt2, wt);
    assert_eq!(git(&wt, &["rev-parse", "HEAD"]), new_head);
    assert!(
        !wt.join("scratch.txt").exists(),
        "untracked leftovers are cleaned"
    );
    assert_eq!(
        std::fs::read_to_string(wt.join("README.md")).unwrap(),
        "# demo\n"
    );
    assert_eq!(git(&f.clone, &["rev-parse", "refs/warden/mr/1"]), new_head);
}

#[tokio::test]
async fn prepare_without_head_sha_uses_fetched_ref() {
    let f = fixture();
    let mgr = WorktreeManager::new(&f.root);
    let p = project(&f, ForgeKind::Gitlab);
    let wt = mgr.prepare(&p, &mr(&f, 1, "")).await.unwrap();
    assert_eq!(git(&wt, &["rev-parse", "HEAD"]), f.head_sha);
}

#[tokio::test]
async fn remove_and_prune() {
    let f = fixture();
    let mgr = WorktreeManager::new(&f.root);
    let p = project(&f, ForgeKind::Gitlab);
    let wt = mgr.prepare(&p, &mr(&f, 1, &f.head_sha)).await.unwrap();
    assert!(wt.exists());

    mgr.remove(&p, 1).await.unwrap();
    assert!(!wt.exists());
    let list = git(&f.clone, &["worktree", "list", "--porcelain"]);
    assert!(!list.contains(wt.to_str().unwrap()), "{list}");
    let refs = git(&f.clone, &["for-each-ref", "refs/warden/"]);
    assert!(refs.is_empty(), "MR ref removed: {refs}");

    // removing again is fine
    mgr.remove(&p, 1).await.unwrap();

    // prune forgets a worktree whose directory was deleted by hand
    let wt = mgr.prepare(&p, &mr(&f, 1, &f.head_sha)).await.unwrap();
    std::fs::remove_dir_all(&wt).unwrap();
    assert!(git(&f.clone, &["worktree", "list", "--porcelain"]).contains(wt.to_str().unwrap()));
    mgr.prune(&p).await.unwrap();
    assert!(!git(&f.clone, &["worktree", "list", "--porcelain"]).contains(wt.to_str().unwrap()));
}

#[tokio::test]
async fn prepare_recovers_from_stale_and_vanished_dirs() {
    let f = fixture();
    let mgr = WorktreeManager::new(&f.root);
    let p = project(&f, ForgeKind::Gitlab);

    // a stale non-worktree directory in the way
    let wt_path = mgr.worktree_path(&p, 1);
    std::fs::create_dir_all(&wt_path).unwrap();
    std::fs::write(wt_path.join("junk"), "x").unwrap();
    let wt = mgr.prepare(&p, &mr(&f, 1, &f.head_sha)).await.unwrap();
    assert_eq!(git(&wt, &["rev-parse", "HEAD"]), f.head_sha);
    assert!(!wt.join("junk").exists());

    // directory deleted by hand while still registered: prepare re-adds it
    std::fs::remove_dir_all(&wt).unwrap();
    let wt = mgr.prepare(&p, &mr(&f, 1, &f.head_sha)).await.unwrap();
    assert_eq!(git(&wt, &["rev-parse", "HEAD"]), f.head_sha);
}

#[tokio::test]
async fn github_uses_pull_refs() {
    let f = fixture();
    let mgr = WorktreeManager::new(&f.root);
    let p = project(&f, ForgeKind::Github);
    let wt = mgr.prepare(&p, &mr(&f, 2, &f.head_sha)).await.unwrap();
    assert!(wt.ends_with(format!("{}/2", project_dir_name(&p))));
    assert_eq!(git(&wt, &["rev-parse", "HEAD"]), f.head_sha);
    assert_eq!(
        git(&f.clone, &["rev-parse", "refs/warden/mr/2"]),
        f.head_sha
    );
}

#[tokio::test]
async fn missing_mr_ref_is_a_git_error() {
    let f = fixture();
    let mgr = WorktreeManager::new(&f.root);
    let p = project(&f, ForgeKind::Gitlab);
    let err = mgr
        .prepare(&p, &mr(&f, 99, "0123456789abcdef0123456789abcdef01234567"))
        .await
        .unwrap_err();
    assert!(matches!(err, warden_core::WardenError::Git(_)), "{err}");
    assert!(err.to_string().contains("merge-requests/99"), "{err}");
}

#[tokio::test]
async fn missing_local_path_is_an_error() {
    let f = fixture();
    let mgr = WorktreeManager::new(&f.root);
    let mut p = project(&f, ForgeKind::Gitlab);
    p.local_path = f.root.join("does-not-exist").to_string_lossy().into_owned();
    assert!(mgr.prepare(&p, &mr(&f, 1, &f.head_sha)).await.is_err());
    p.local_path = String::new();
    assert!(matches!(
        mgr.prepare(&p, &mr(&f, 1, &f.head_sha)).await.unwrap_err(),
        warden_core::WardenError::Config(_)
    ));
}

/// The clone's `origin` is the user's fork (no MR refs); `upstream` is the GitLab project whose URL
/// matches `project.base_url`. Its https URL is redirected to the local bare repo with `insteadOf`.
#[tokio::test]
async fn prefers_remote_matching_base_url() {
    let f = fixture();
    let base = f.remote.parent().unwrap().to_path_buf();
    let fork = base.join("fork.git");
    git(
        &base,
        &[
            "clone",
            "-q",
            "--bare",
            f.remote.to_str().unwrap(),
            fork.to_str().unwrap(),
        ],
    );
    git(&fork, &["update-ref", "-d", "refs/merge-requests/1/head"]);
    git(
        &f.clone,
        &["remote", "set-url", "origin", fork.to_str().unwrap()],
    );
    let upstream_url = "https://gitlab.example.com/entries/demo.git";
    git(&f.clone, &["remote", "add", "upstream", upstream_url]);
    git(
        &f.clone,
        &[
            "config",
            &format!("url.file://{}.insteadOf", f.remote.display()),
            upstream_url,
        ],
    );

    let mgr = WorktreeManager::new(&f.root);
    let p = project(&f, ForgeKind::Gitlab);
    // A different head than anything already in the clone, reachable only through upstream's MR ref.
    std::fs::write(f.seed.join("only_upstream.py"), "x = 1\n").unwrap();
    git(&f.seed, &["add", "."]);
    git(&f.seed, &["commit", "-q", "-m", "only upstream"]);
    let head = git(&f.seed, &["rev-parse", "HEAD"]);
    git(
        &f.seed,
        &["push", "-q", "origin", "HEAD:refs/heads/mr-only"],
    );
    git(
        &f.remote,
        &["update-ref", "refs/merge-requests/1/head", &head],
    );

    let wt = mgr.prepare(&p, &mr(&f, 1, &head)).await.unwrap();
    assert_eq!(git(&wt, &["rev-parse", "HEAD"]), head);
    assert!(wt.join("only_upstream.py").exists());
}

#[tokio::test]
async fn tilde_root_is_expanded() {
    let mgr = WorktreeManager::new("~/.warden/worktrees");
    let home = PathBuf::from(std::env::var_os("HOME").unwrap());
    let f = fixture();
    let p = project(&f, ForgeKind::Gitlab);
    assert_eq!(
        mgr.worktree_path(&p, 7),
        home.join(".warden/worktrees")
            .join(project_dir_name(&p))
            .join("7")
    );
}

/// Security regression: a forge response whose sha is really a git option must never reach git
/// (`git fetch <remote> --upload-pack=<cmd>` would run <cmd>).
#[tokio::test]
async fn option_like_forge_shas_are_refused() {
    let f = fixture();
    let mgr = WorktreeManager::new(&f.root);
    let p = project(&f, ForgeKind::Gitlab);
    let marker = f.root.parent().unwrap().join("pwned");
    let evil = format!("--upload-pack=touch {};false", marker.display());

    // MR ref missing -> the old code fell back to `git fetch <remote> <head_sha>`.
    let err = mgr.prepare(&p, &mr(&f, 99, &evil)).await.unwrap_err();
    assert!(err.to_string().contains("not a hex commit sha"), "{err}");

    let mut bad_base = mr(&f, 1, &f.head_sha);
    bad_base.base_sha = evil.clone();
    assert!(mgr.prepare(&p, &bad_base).await.is_err());
    let mut bad_start = mr(&f, 1, &f.head_sha);
    bad_start.start_sha = evil;
    assert!(mgr.prepare(&p, &bad_start).await.is_err());
    assert!(!marker.exists(), "git ran the injected upload-pack command");
}
