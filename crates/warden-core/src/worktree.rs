// OWNER: ci
//! git worktree management: one detached worktree per MR under `<root>/<project>/<iid>`.
//!
//! All git work goes through `tokio::process` (`git` must be on PATH). Every git invocation runs
//! with `GIT_TERMINAL_PROMPT=0` so a missing credential fails fast instead of hanging on a prompt,
//! and with `LC_ALL=C` so error messages are predictable.
//!
//! Flow of [`WorktreeManager::prepare`]:
//! 1. pick the remote in `project.local_path`: a remote whose URL matches `project.base_url`'s host
//!    (and preferably `project.remote_id`), else `origin`, else the first remote;
//! 2. `git fetch <remote> +refs/merge-requests/<iid>/head:refs/warden/mr/<iid>` (GitLab) or
//!    `+refs/pull/<iid>/head:refs/warden/mr/<iid>` (GitHub). If that fails and `head_sha` is not
//!    already present, try `git fetch <remote> <head_sha>`;
//! 3. make sure `base_sha` is present (best effort fetch of the target branch / the sha);
//! 4. `git worktree add --detach <root>/<sanitized project name>/<iid> <head_sha>`, or, when the
//!    worktree already exists, `git -C <wt> checkout --force --detach <head_sha>` + `git clean -fd`.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use tokio::process::Command;

use crate::error::{Result, WardenError};
use crate::types::{ForgeKind, MergeRequest, Project};

/// Network operations (fetch) get this long before they are killed.
const FETCH_TIMEOUT: Duration = Duration::from_secs(300);
/// Local operations (worktree add, diff, ...) get this long.
const LOCAL_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Debug, Clone)]
pub struct WorktreeManager {
    pub root: PathBuf,
    /// Extra environment for `git fetch` (auth header for Warden-managed clones, see
    /// `discovery::auth_env`). Empty for the user's own clones.
    pub fetch_env: Vec<(String, String)>,
}

impl WorktreeManager {
    /// `root` may start with `~` (expanded against `$HOME`).
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            fetch_env: Vec::new(),
        }
    }

    /// Same manager, with extra environment for fetches.
    pub fn with_fetch_env(mut self, env: Vec<(String, String)>) -> Self {
        self.fetch_env = env;
        self
    }

    /// The root with `~` expanded.
    pub fn root_dir(&self) -> PathBuf {
        expand_tilde(&self.root)
    }

    /// `<root>/<sanitized project name>/<iid>`
    pub fn worktree_path(&self, project: &Project, iid: u64) -> PathBuf {
        self.project_dir(project).join(iid.to_string())
    }

    /// `<root>/<sanitized project name>-<hash of base_url + remote_id>`. The hash keeps projects
    /// with the same name apart (`group-a/crm` vs `group-b/crm`, or gitlab.com vs self-hosted).
    pub fn project_dir(&self, project: &Project) -> PathBuf {
        self.root_dir().join(project_dir_name(project))
    }

    /// Fetch MR refs (gitlab: `refs/merge-requests/<iid>/head`, github: `pull/<n>/head`) into the
    /// project's `local_path` repo, then `git worktree add --detach <root>/<project>/<iid> <head_sha>`
    /// (or update the existing worktree to `head_sha`).
    pub async fn prepare(&self, project: &Project, mr: &MergeRequest) -> Result<PathBuf> {
        // The shas come from the forge API: never let one be parsed as a git option.
        for (what, sha) in [
            ("head_sha", &mr.head_sha),
            ("base_sha", &mr.base_sha),
            ("start_sha", &mr.start_sha),
        ] {
            if !sha.is_empty() && !is_hex_sha(sha) {
                return Err(WardenError::Git(format!(
                    "refusing {what} {sha:?} of MR !{}: not a hex commit sha",
                    mr.iid
                )));
            }
        }
        if mr.target_branch.starts_with('-') {
            return Err(WardenError::Git(format!(
                "refusing target branch {:?} of MR !{}",
                mr.target_branch, mr.iid
            )));
        }
        let repo = repo_path(project)?;
        let remote = pick_remote(&repo, project).await?;
        let local_ref = format!("refs/warden/mr/{}", mr.iid);
        let src_ref = match project.forge {
            ForgeKind::Gitlab => format!("refs/merge-requests/{}/head", mr.iid),
            ForgeKind::Github => format!("refs/pull/{}/head", mr.iid),
        };
        let refspec = format!("+{src_ref}:{local_ref}");

        let fetch = git_env_timeout(
            &repo,
            &["fetch", "--no-tags", "--end-of-options", &remote, &refspec],
            FETCH_TIMEOUT,
            &self.fetch_env,
        )
        .await;
        if let Err(fetch_err) = fetch {
            // The MR ref may be gone or unreachable; the commit itself may still be obtainable.
            let have_head = !mr.head_sha.is_empty() && has_commit(&repo, &mr.head_sha).await;
            if !have_head {
                let by_sha = if mr.head_sha.is_empty() {
                    Err(fetch_err)
                } else {
                    git_env_timeout(
                        &repo,
                        &[
                            "fetch",
                            "--no-tags",
                            "--end-of-options",
                            &remote,
                            &mr.head_sha,
                        ],
                        FETCH_TIMEOUT,
                        &self.fetch_env,
                    )
                    .await
                    .map_err(|_| fetch_err)
                };
                if let Err(e) = by_sha {
                    return Err(WardenError::Git(format!(
                        "could not fetch {src_ref} from remote '{remote}': {e}"
                    )));
                }
            }
        }

        // Resolve the commit to check out.
        let head = if mr.head_sha.is_empty() {
            git(
                &repo,
                &["rev-parse", "--verify", &format!("{local_ref}^{{commit}}")],
            )
            .await?
            .trim()
            .to_string()
        } else {
            mr.head_sha.clone()
        };
        if !has_commit(&repo, &head).await {
            return Err(WardenError::Git(format!(
                "head commit {head} of MR !{} is not available after fetching from '{remote}'",
                mr.iid
            )));
        }

        // The diff needs the base commit too (best effort: the pipeline reports diff errors).
        ensure_base(&repo, &remote, mr, &self.fetch_env).await;

        let wt = self.worktree_path(project, mr.iid);
        if wt.join(".git").exists() {
            match update_worktree(&wt, &head).await {
                Ok(()) => return Ok(wt),
                Err(e) => {
                    tracing::warn!(
                        "worktree update failed at {}, recreating: {e}",
                        wt.display()
                    );
                    let _ = git(&repo, &["worktree", "remove", "--force", path_str(&wt)?]).await;
                    remove_dir_if_exists(&wt).await?;
                }
            }
        } else if wt.exists() {
            // A stale directory that is not a worktree (e.g. a crashed add): replace it.
            remove_dir_if_exists(&wt).await?;
        }

        // Drop registrations whose directories vanished, otherwise `worktree add` refuses the path.
        git(&repo, &["worktree", "prune"]).await?;
        if let Some(parent) = wt.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        git(
            &repo,
            &[
                "worktree",
                "add",
                "--force",
                "--detach",
                "--end-of-options",
                path_str(&wt)?,
                &head,
            ],
        )
        .await?;
        Ok(wt)
    }

    /// `git diff --no-color --no-ext-diff -U3 base...head` (falls back to `base..head` when no
    /// merge base exists). Prefixes are forced to `a/` and `b/` and paths are not quoted, whatever
    /// the user's git config says, so the output is stable for the diff parser.
    pub async fn diff(&self, worktree: &Path, base: &str, head: &str) -> Result<String> {
        let three_dot = format!("{base}...{head}");
        match git(worktree, &diff_args(&three_dot)).await {
            Ok(out) => Ok(out),
            Err(first) => {
                let two_dot = format!("{base}..{head}");
                git(worktree, &diff_args(&two_dot)).await.map_err(|e| {
                    WardenError::Git(format!("{first}; fallback {two_dot} failed: {e}"))
                })
            }
        }
    }

    /// `git worktree remove --force <wt>` + `git worktree prune`; also drops `refs/warden/mr/<iid>`.
    /// Removing a worktree that does not exist is not an error.
    pub async fn remove(&self, project: &Project, iid: u64) -> Result<()> {
        let repo = repo_path(project)?;
        let wt = self.worktree_path(project, iid);
        if wt.exists() {
            if let Err(e) = git(&repo, &["worktree", "remove", "--force", path_str(&wt)?]).await {
                tracing::debug!("git worktree remove failed ({e}); deleting directory");
                remove_dir_if_exists(&wt).await?;
            }
        }
        git(&repo, &["worktree", "prune"]).await?;
        let _ = git(
            &repo,
            &["update-ref", "-d", &format!("refs/warden/mr/{iid}")],
        )
        .await;
        // Tidy an empty project directory.
        let _ = tokio::fs::remove_dir(self.project_dir(project)).await;
        Ok(())
    }

    /// `git worktree prune` in the project's repo (forgets worktrees whose directories are gone).
    pub async fn prune(&self, project: &Project) -> Result<()> {
        let repo = repo_path(project)?;
        git(&repo, &["worktree", "prune"]).await?;
        Ok(())
    }
}

fn diff_args(range: &str) -> Vec<&str> {
    vec![
        "-c",
        "core.quotePath=false",
        "diff",
        "--no-color",
        "--no-ext-diff",
        "--no-textconv",
        "--src-prefix=a/",
        "--dst-prefix=b/",
        "-U3",
        range,
    ]
}

async fn update_worktree(wt: &Path, head: &str) -> Result<()> {
    if !is_hex_sha(head) {
        return Err(WardenError::Git(format!("refusing to check out {head:?}")));
    }
    git(wt, &["checkout", "--force", "--detach", head, "--"]).await?;
    // Remove untracked files left by a previous run (ignored files such as node_modules stay).
    git(wt, &["clean", "-fd"]).await?;
    Ok(())
}

async fn ensure_base(repo: &Path, remote: &str, mr: &MergeRequest, env: &[(String, String)]) {
    for sha in [&mr.base_sha, &mr.start_sha] {
        if sha.is_empty() || has_commit(repo, sha).await {
            continue;
        }
        if !mr.target_branch.is_empty() {
            let spec = format!("+refs/heads/{0}:refs/warden/target/{0}", mr.target_branch);
            let _ = git_env_timeout(
                repo,
                &["fetch", "--no-tags", "--end-of-options", remote, &spec],
                FETCH_TIMEOUT,
                env,
            )
            .await;
        }
        if is_hex_sha(sha) && !has_commit(repo, sha).await {
            let _ = git_env_timeout(
                repo,
                &["fetch", "--no-tags", "--end-of-options", remote, sha],
                FETCH_TIMEOUT,
                env,
            )
            .await;
        }
    }
}

async fn has_commit(repo: &Path, sha: &str) -> bool {
    if !is_hex_sha(sha) {
        return false;
    }
    git(repo, &["cat-file", "-e", &format!("{sha}^{{commit}}")])
        .await
        .is_ok()
}

fn repo_path(project: &Project) -> Result<PathBuf> {
    if project.local_path.trim().is_empty() {
        return Err(WardenError::Config(format!(
            "project '{}' has no local_path",
            project.name
        )));
    }
    let p = expand_tilde(Path::new(project.local_path.trim()));
    if !p.is_dir() {
        return Err(WardenError::Git(format!(
            "local_path {} of project '{}' is not a directory",
            p.display(),
            project.name
        )));
    }
    Ok(p)
}

/// Remote whose URL matches the project's forge, else `origin`, else the first remote.
async fn pick_remote(repo: &Path, project: &Project) -> Result<String> {
    let out = git(repo, &["remote"]).await?;
    let remotes: Vec<String> = out
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect();
    if remotes.is_empty() {
        return Err(WardenError::Git(format!(
            "repository {} has no git remotes",
            repo.display()
        )));
    }
    let mut urls = Vec::with_capacity(remotes.len());
    for r in &remotes {
        // Raw configured URL (`remote get-url` would apply `insteadOf` rewrites).
        let key = format!("remote.{r}.url");
        let url = git(repo, &["config", "--get", &key])
            .await
            .unwrap_or_default();
        urls.push((r.clone(), url.trim().to_string()));
    }
    Ok(choose_remote(&urls, &project.base_url, &project.remote_id))
}

/// Pure remote selection (unit tested). `remotes` = (name, url).
pub(crate) fn choose_remote(
    remotes: &[(String, String)],
    base_url: &str,
    remote_id: &str,
) -> String {
    let host = url_host(base_url);
    let path = remote_id.trim().trim_matches('/').to_ascii_lowercase();
    let path_matches = |url: &str| {
        !path.is_empty() && !path.chars().all(|c| c.is_ascii_digit()) && {
            let u = url.to_ascii_lowercase();
            let u = u.trim_end_matches('/').trim_end_matches(".git");
            u.ends_with(&format!("/{path}")) || u.ends_with(&format!(":{path}"))
        }
    };
    if let Some(host) = host.as_deref() {
        let on_host: Vec<&(String, String)> = remotes
            .iter()
            .filter(|(_, url)| url_host(url).as_deref() == Some(host))
            .collect();
        if let Some((name, _)) = on_host.iter().find(|(_, url)| path_matches(url)) {
            return name.clone();
        }
        if let Some((name, _)) = on_host.iter().find(|(n, _)| n == "origin") {
            return name.clone();
        }
        if let Some((name, _)) = on_host.first() {
            return name.clone();
        }
    }
    if remotes.iter().any(|(n, _)| n == "origin") {
        return "origin".to_string();
    }
    remotes[0].0.clone()
}

/// Lower-cased host of an http(s)/ssh/scp-style git URL, without user and port.
pub(crate) fn url_host(url: &str) -> Option<String> {
    let url = url.trim();
    if url.is_empty() {
        return None;
    }
    let rest = if let Some((scheme, rest)) = url.split_once("://") {
        if scheme.eq_ignore_ascii_case("file") {
            return None;
        }
        rest.split('/').next().unwrap_or("")
    } else if let Some((before, _)) = url.split_once(':') {
        // scp-like `git@host:group/repo.git` (but not a Windows drive or a local path)
        if before.contains('/') || before.len() <= 1 {
            return None;
        }
        before
    } else {
        return None;
    };
    let rest = rest.rsplit('@').next().unwrap_or(rest);
    let host = if rest.starts_with('[') {
        rest.split(']')
            .next()
            .map(|h| format!("{h}]"))
            .unwrap_or_default()
    } else {
        rest.split(':').next().unwrap_or("").to_string()
    };
    let host = host.to_ascii_lowercase();
    (!host.is_empty()).then_some(host)
}

/// True for a 7..=64 character lower/upper-case hex string (an abbreviated or full commit sha).
/// Forge-supplied shas must pass this before they reach a git command line.
pub fn is_hex_sha(s: &str) -> bool {
    (7..=64).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// `<sanitized name>-<first 10 hex of sha256(base_url \n remote_id)>`.
pub fn project_dir_name(project: &Project) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(project.base_url.trim().trim_end_matches('/').as_bytes());
    h.update(b"\n");
    h.update(project.remote_id.trim().as_bytes());
    let tag = hex::encode(h.finalize());
    format!("{}-{}", sanitize_name(&project.name), &tag[..10])
}

/// Directory-safe project name: `[A-Za-z0-9._-]` kept, everything else becomes `_`.
pub fn sanitize_name(name: &str) -> String {
    let s: String = name
        .trim()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let trimmed = s.trim_matches('.');
    if trimmed.is_empty() {
        "project".to_string()
    } else {
        trimmed.to_string()
    }
}

/// Expands a leading `~` / `~/` against `$HOME`.
pub fn expand_tilde(p: &Path) -> PathBuf {
    let s = p.to_string_lossy();
    if s == "~" || s.starts_with("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            let home = PathBuf::from(home);
            return if s == "~" { home } else { home.join(&s[2..]) };
        }
    }
    p.to_path_buf()
}

fn path_str(p: &Path) -> Result<&str> {
    p.to_str()
        .ok_or_else(|| WardenError::Git(format!("non UTF-8 path: {}", p.display())))
}

async fn remove_dir_if_exists(p: &Path) -> Result<()> {
    match tokio::fs::remove_dir_all(p).await {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

async fn git(dir: &Path, args: &[&str]) -> Result<String> {
    git_timeout(dir, args, LOCAL_TIMEOUT).await
}

/// Runs `git -C <dir> <args>`; returns stdout, or `WardenError::Git` with stderr on failure.
async fn git_timeout(dir: &Path, args: &[&str], timeout: Duration) -> Result<String> {
    git_env_timeout(dir, args, timeout, &[]).await
}

async fn git_env_timeout(
    dir: &Path,
    args: &[&str],
    timeout: Duration,
    env: &[(String, String)],
) -> Result<String> {
    let mut cmd = Command::new("git");
    for (k, v) in env {
        cmd.env(k, v);
    }
    cmd.arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("LC_ALL", "C")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let child = cmd
        .spawn()
        .map_err(|e| WardenError::Git(format!("cannot run git: {e}")))?;
    let out = match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(r) => r.map_err(|e| WardenError::Git(format!("git {}: {e}", args.join(" "))))?,
        Err(_) => {
            return Err(WardenError::Git(format!(
                "git {} timed out after {}s",
                args.join(" "),
                timeout.as_secs()
            )))
        }
    };
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        let stderr = String::from_utf8_lossy(&out.stderr);
        Err(WardenError::Git(format!(
            "git {} failed ({}): {}",
            args.join(" "),
            out.status,
            stderr.trim()
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(name: &str, url: &str) -> (String, String) {
        (name.to_string(), url.to_string())
    }

    #[test]
    fn host_parsing() {
        assert_eq!(
            url_host("https://gitlab.com").as_deref(),
            Some("gitlab.com")
        );
        assert_eq!(
            url_host("https://user:pw@GitLab.Example.com:8443/a/b.git").as_deref(),
            Some("gitlab.example.com")
        );
        assert_eq!(
            url_host("git@gitlab.com:entries/crm.git").as_deref(),
            Some("gitlab.com")
        );
        assert_eq!(
            url_host("ssh://git@gitlab.com:2222/entries/crm.git").as_deref(),
            Some("gitlab.com")
        );
        assert_eq!(url_host("/tmp/repo.git"), None);
        assert_eq!(url_host("file:///tmp/repo.git"), None);
        assert_eq!(url_host("../relative/repo"), None);
        assert_eq!(url_host(""), None);
    }

    #[test]
    fn remote_choice_prefers_matching_host_then_origin() {
        let remotes = vec![
            r("origin", "git@github.com:me/crm.git"),
            r("upstream", "https://gitlab.com/entries/entries_crm.git"),
        ];
        assert_eq!(
            choose_remote(&remotes, "https://gitlab.com", "entries/entries_crm"),
            "upstream"
        );
        assert_eq!(
            choose_remote(&remotes, "https://gitlab.com", "42"),
            "upstream"
        );
        assert_eq!(
            choose_remote(&remotes, "https://gitlab.other.io", "x/y"),
            "origin"
        );

        // two remotes on the same host: the one matching remote_id wins
        let remotes = vec![
            r("origin", "git@gitlab.com:me/entries_crm.git"),
            r("upstream", "git@gitlab.com:entries/entries_crm.git"),
        ];
        assert_eq!(
            choose_remote(&remotes, "https://gitlab.com", "entries/entries_crm"),
            "upstream"
        );
        assert_eq!(
            choose_remote(&remotes, "https://gitlab.com", "99"),
            "origin"
        );

        // no origin, no match -> first remote
        let remotes = vec![r("mine", "/tmp/x.git"), r("other", "/tmp/y.git")];
        assert_eq!(choose_remote(&remotes, "https://gitlab.com", "a/b"), "mine");
    }

    #[test]
    fn sanitize() {
        assert_eq!(sanitize_name("entries/entries_crm"), "entries_entries_crm");
        assert_eq!(sanitize_name("My Project!"), "My_Project_");
        assert_eq!(sanitize_name(".."), "project");
        assert_eq!(sanitize_name(""), "project");
        assert_eq!(sanitize_name("a.b-c_d"), "a.b-c_d");
    }

    #[test]
    fn hex_sha_validation() {
        assert!(is_hex_sha("0123456789abcdef0123456789abcdef01234567"));
        assert!(is_hex_sha("abc1234"));
        assert!(!is_hex_sha("--upload-pack=touch /tmp/pwned"));
        assert!(!is_hex_sha("-abc1234"));
        assert!(!is_hex_sha("abc12"));
        assert!(!is_hex_sha("main"));
        assert!(!is_hex_sha(""));
    }

    fn project(name: &str, base_url: &str, remote_id: &str) -> Project {
        Project {
            id: 1,
            forge: ForgeKind::Gitlab,
            base_url: base_url.into(),
            remote_id: remote_id.into(),
            name: name.into(),
            local_path: "/tmp/x".into(),
            target_branches: vec![],
            mode: crate::types::PostingMode::Draft,
            poll_interval_secs: 60,
            enabled: true,
        }
    }

    #[test]
    fn same_named_projects_get_distinct_dirs() {
        let m = WorktreeManager::new("/wt");
        let a = project("crm", "https://gitlab.com", "group-a/crm");
        let b = project("crm", "https://gitlab.com", "group-b/crm");
        let c = project("crm", "https://git.example.com", "group-a/crm");
        let pa = m.worktree_path(&a, 3);
        assert_ne!(pa, m.worktree_path(&b, 3));
        assert_ne!(pa, m.worktree_path(&c, 3));
        assert_eq!(pa, m.worktree_path(&a.clone(), 3));
        assert!(pa
            .parent()
            .unwrap()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("crm-"));
    }

    #[test]
    fn tilde() {
        let home = PathBuf::from(std::env::var_os("HOME").unwrap());
        assert_eq!(
            expand_tilde(Path::new("~/.warden/worktrees")),
            home.join(".warden/worktrees")
        );
        assert_eq!(expand_tilde(Path::new("~")), home);
        assert_eq!(expand_tilde(Path::new("/abs/~x")), PathBuf::from("/abs/~x"));
        let m = WorktreeManager::new("~/wt");
        assert_eq!(m.root_dir(), home.join("wt"));
    }
}
