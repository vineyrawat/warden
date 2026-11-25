// OWNER: integration
//! Project discovery for the add-project screen: find git clones on this Mac, match them to forge
//! projects by remote URL, parse pasted MR links, and make Warden-managed clones.
//!
//! Security notes:
//! - The scan only reads `.git/config` files; it never runs git inside scanned folders (a repo's
//!   config could name hooks or helpers).
//! - Managed clones authenticate with an `http.<forge-origin>/.extraHeader` passed through
//!   `GIT_CONFIG_*` environment variables: the token never lands in a command line, a remote URL
//!   or a config file, and git only sends it to URLs under the forge origin.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use tokio::process::Command;

use crate::error::{Result, WardenError};
use crate::types::{
    DiscoveredProject, Discovery, ForgeKind, LocalRepo, MrLink, Project, RemoteProjectInfo,
};
use crate::worktree::{expand_tilde, project_dir_name};

/// Folders under `$HOME` scanned when the user has not picked one.
pub const DEFAULT_ROOTS: &[&str] = &[
    "Desktop",
    "Developer",
    "code",
    "Code",
    "src",
    "dev",
    "Projects",
    "projects",
    "workspace",
    "work",
    "git",
    "repos",
    "Sites",
];

/// Directory names never descended into.
const SKIP_DIRS: &[&str] = &[
    "node_modules",
    "target",
    "dist",
    "build",
    "env",
    "venv",
    ".venv",
    "__pycache__",
    "sites",
    "Library",
    "Applications",
    "Pictures",
    "Music",
    "Movies",
    "logs",
    "vendor",
    "Pods",
];

const MAX_DEPTH: usize = 6;
const MAX_DIRS: u32 = 40_000;
const SCAN_BUDGET: Duration = Duration::from_secs(8);
const CLONE_TIMEOUT: Duration = Duration::from_secs(15 * 60);

// ---------------------------------------------------------------------------------------------
// remote URL normalisation
// ---------------------------------------------------------------------------------------------

/// `(host, path)` for a git remote URL, lower-cased, without `.git`, credentials or port.
/// Handles `git@host:group/repo.git`, `ssh://git@host:22/group/repo.git`,
/// `https://user:pw@host/group/repo(.git)` and `git://host/...`.
pub fn normalize_remote(url: &str) -> Option<(String, String)> {
    let url = url.trim();
    if url.is_empty() {
        return None;
    }
    let (host, path) = if let Some(rest) = url.split_once("://").map(|(_, r)| r) {
        let (authority, path) = rest.split_once('/')?;
        let host = authority.rsplit('@').next()?;
        let host = host.split(':').next()?;
        (host.to_string(), path.to_string())
    } else if let Some((left, path)) = url.split_once(':') {
        // scp-like syntax; a bare Windows/local path has no '@' or dot in `left`.
        if left.contains('/') {
            return None;
        }
        let host = left.rsplit('@').next()?;
        (host.to_string(), path.to_string())
    } else {
        return None;
    };
    let host = host.trim().to_ascii_lowercase();
    let path = path
        .trim()
        .trim_matches('/')
        .trim_end_matches(".git")
        .trim_matches('/')
        .to_ascii_lowercase();
    if host.is_empty() || path.is_empty() {
        return None;
    }
    Some((host, path))
}

/// Host of a base URL such as `https://gitlab.com` or `https://git.example.com:8443/gitlab`.
pub fn base_host(base_url: &str) -> Option<String> {
    let rest = base_url.trim().split_once("://").map(|(_, r)| r)?;
    let authority = rest.split('/').next()?;
    let host = authority.rsplit('@').next()?.split(':').next()?;
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}

/// Path prefix of a self-hosted forge under a sub-path (`https://host/gitlab` → `gitlab`).
fn base_prefix(base_url: &str) -> String {
    base_url
        .trim()
        .split_once("://")
        .map(|(_, r)| r)
        .and_then(|r| r.split_once('/'))
        .map(|(_, p)| p.trim_matches('/').to_ascii_lowercase())
        .unwrap_or_default()
}

/// The forge project path a remote URL points at, when it is on `base_url`'s host.
pub fn remote_id_for(url: &str, base_url: &str) -> Option<String> {
    let (host, path) = normalize_remote(url)?;
    if Some(host) != base_host(base_url) {
        return None;
    }
    let prefix = base_prefix(base_url);
    let path = if !prefix.is_empty() {
        path.strip_prefix(&format!("{prefix}/"))
            .map(str::to_string)
            .unwrap_or(path)
    } else {
        path
    };
    Some(path)
}

// ---------------------------------------------------------------------------------------------
// folder scan
// ---------------------------------------------------------------------------------------------

/// A git repo found on disk: its working-tree path and `(remote name, url)` pairs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScannedRepo {
    pub path: PathBuf,
    pub remotes: Vec<(String, String)>,
}

/// Existing default roots under `$HOME`.
pub fn default_roots() -> Vec<PathBuf> {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return Vec::new();
    };
    // `Projects` and `projects` are the same folder on a case-insensitive disk: dedupe by inode.
    let mut seen = HashSet::new();
    DEFAULT_ROOTS
        .iter()
        .map(|d| home.join(d))
        .filter(|p| p.is_dir() && file_id(p).is_none_or(|id| seen.insert(id)))
        .collect()
}

/// Walks `roots` (depth ≤ 6, ≤ 40k folders, ≤ 8 s) and returns every git working tree found.
/// Does not descend into a repo once found, nor into hidden or build/dependency folders.
pub fn scan_repos(roots: &[PathBuf]) -> (Vec<ScannedRepo>, u32) {
    let started = Instant::now();
    let mut visited = 0u32;
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for root in roots {
        let root = expand_tilde(root);
        let mut stack = vec![(root, 0usize)];
        while let Some((dir, depth)) = stack.pop() {
            if visited >= MAX_DIRS || started.elapsed() > SCAN_BUDGET {
                tracing::warn!(visited, "folder scan stopped early");
                return (out, visited);
            }
            visited += 1;
            if let Some(id) = file_id(&dir) {
                if !seen.insert(id) {
                    continue;
                }
            }
            let git_dir = dir.join(".git");
            if git_dir.is_dir() {
                let remotes = read_remotes(&git_dir.join("config"));
                if !remotes.is_empty() {
                    out.push(ScannedRepo {
                        path: dir.clone(),
                        remotes,
                    });
                }
                continue;
            }
            if depth >= MAX_DEPTH {
                continue;
            }
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            let mut children: Vec<PathBuf> = entries
                .filter_map(|e| e.ok())
                .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false)) // no symlinks
                .filter(|e| {
                    let name = e.file_name();
                    let name = name.to_string_lossy();
                    !name.starts_with('.') && !SKIP_DIRS.contains(&name.as_ref())
                })
                .map(|e| e.path())
                .collect();
            children.sort();
            for c in children.into_iter().rev() {
                stack.push((c, depth + 1));
            }
        }
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    (out, visited)
}

/// `(device, inode)`: identifies a folder whatever case or symlinks were used to reach it.
fn file_id(p: &Path) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(p).ok().map(|m| (m.dev(), m.ino()))
}

/// `(name, url)` of each `[remote "name"]` in a git config file. Minimal INI parsing: enough for
/// the `url = ...` lines git writes itself.
pub fn read_remotes(config: &Path) -> Vec<(String, String)> {
    std::fs::read_to_string(config)
        .map(|s| parse_remotes(&s))
        .unwrap_or_default()
}

pub fn parse_remotes(config: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut current: Option<String> = None;
    for line in config.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            current = line
                .strip_prefix("[remote \"")
                .and_then(|r| r.strip_suffix("\"]"))
                .map(str::to_string);
            continue;
        }
        let Some(name) = &current else { continue };
        if let Some((k, v)) = line.split_once('=') {
            if k.trim().eq_ignore_ascii_case("url") {
                let v = v.trim().trim_matches('"').to_string();
                if !v.is_empty() && !out.iter().any(|(n, _): &(String, String)| n == name) {
                    out.push((name.clone(), v));
                }
            }
        }
    }
    out
}

/// Merges remote projects with scanned repos. Paths are shown with `~` for `$HOME`.
pub fn merge(
    base_url: &str,
    remote: Vec<RemoteProjectInfo>,
    scanned: &[ScannedRepo],
    existing: &[Project],
) -> (Vec<DiscoveredProject>, Vec<LocalRepo>) {
    let home = std::env::var("HOME").unwrap_or_default();
    let tilde = |p: &Path| {
        let s = p.to_string_lossy().to_string();
        if !home.is_empty() && s.starts_with(&home) {
            format!("~{}", &s[home.len()..])
        } else {
            s
        }
    };

    let mut by_id: HashMap<String, Vec<String>> = HashMap::new();
    let mut local = Vec::new();
    for repo in scanned {
        // Prefer a remote on this forge; origin first.
        let mut remotes = repo.remotes.clone();
        remotes.sort_by_key(|(n, _)| n != "origin");
        let hit = remotes
            .iter()
            .find_map(|(_, url)| remote_id_for(url, base_url).map(|id| (id, url.clone())));
        let path = tilde(&repo.path);
        match hit {
            Some((id, url)) => {
                let dup = by_id.get(&id).and_then(|v| v.first().cloned());
                by_id.entry(id.clone()).or_default().push(path.clone());
                local.push(LocalRepo {
                    path,
                    remote_url: url,
                    remote_id: Some(id),
                    duplicate_of: dup,
                });
            }
            None => local.push(LocalRepo {
                path,
                remote_url: remotes.first().map(|(_, u)| u.clone()).unwrap_or_default(),
                remote_id: None,
                duplicate_of: None,
            }),
        }
    }

    let base = base_url.trim().trim_end_matches('/');
    let existing_id = |remote_id: &str| {
        existing
            .iter()
            .find(|p| {
                p.base_url.trim_end_matches('/') == base
                    && p.remote_id.eq_ignore_ascii_case(remote_id)
            })
            .map(|p| p.id)
    };

    let mut projects: Vec<DiscoveredProject> = remote
        .into_iter()
        .map(|r| {
            let key = r.remote_id.to_ascii_lowercase();
            DiscoveredProject {
                local_paths: by_id.get(&key).cloned().unwrap_or_default(),
                existing_project_id: existing_id(&r.remote_id),
                remote: r,
            }
        })
        .collect();
    // Projects with a local clone first, keeping the forge's activity order inside each group.
    projects.sort_by_key(|p| p.local_paths.is_empty());
    (projects, local)
}

// ---------------------------------------------------------------------------------------------
// MR links
// ---------------------------------------------------------------------------------------------

/// Parses `https://host[/prefix]/group/sub/repo/-/merge_requests/41[...]` (GitLab) and
/// `https://github.com/owner/repo/pull/12[...]` (GitHub, incl. GHE). The base URL is the part
/// before the project path; for GitLab sub-path installs pass `known_bases` to split correctly.
pub fn parse_mr_link(url: &str, known_bases: &[(ForgeKind, String)]) -> Option<MrLink> {
    let url = url.trim();
    let url = url.split(['?', '#']).next()?;
    let (scheme, rest) = url.split_once("://")?;
    if scheme != "https" && scheme != "http" {
        return None;
    }
    let mut parts = rest.trim_end_matches('/').splitn(2, '/');
    let host = parts.next()?.to_string();
    let path = parts.next()?;

    if let Some((proj, tail)) = path.split_once("/-/merge_requests/") {
        let iid: u64 = tail.split('/').next()?.parse().ok()?;
        let origin = format!("{scheme}://{host}");
        // Longest known base on this host whose prefix the path starts with.
        let mut base = origin.clone();
        let mut proj = proj.to_string();
        for (kind, b) in known_bases {
            if *kind != ForgeKind::Gitlab {
                continue;
            }
            let b = b.trim_end_matches('/');
            if let Some(sub) = b.strip_prefix(&format!("{origin}/")) {
                if let Some(p) = proj.strip_prefix(&format!("{sub}/")) {
                    base = b.to_string();
                    proj = p.to_string();
                }
            }
        }
        if proj.is_empty() {
            return None;
        }
        return Some(MrLink {
            forge: ForgeKind::Gitlab,
            base_url: base,
            remote_id: proj,
            iid,
        });
    }

    let segs: Vec<&str> = path.split('/').collect();
    if segs.len() >= 4 && segs[2] == "pull" {
        let iid: u64 = segs[3].parse().ok()?;
        return Some(MrLink {
            forge: ForgeKind::Github,
            base_url: format!("{scheme}://{host}"),
            remote_id: format!("{}/{}", segs[0], segs[1]),
            iid,
        });
    }
    None
}

// ---------------------------------------------------------------------------------------------
// managed clones
// ---------------------------------------------------------------------------------------------

/// `Basic base64(user:token)`, the form both GitLab and GitHub accept for git over HTTPS.
pub fn basic_auth_header(user: &str, token: &str) -> String {
    format!("Basic {}", base64(format!("{user}:{token}").as_bytes()))
}

fn base64(input: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            T[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            T[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// `<data_dir>/repos/<project dir name>`.
pub fn managed_clone_dir(data_dir: &Path, project: &Project) -> PathBuf {
    data_dir.join("repos").join(project_dir_name(project))
}

/// True when `local_path` is a Warden-managed clone (under `<data_dir>/repos`).
pub fn is_managed_clone(data_dir: &Path, local_path: &str) -> bool {
    expand_tilde(Path::new(local_path)).starts_with(data_dir.join("repos"))
}

/// The `GIT_CONFIG_*` environment that scopes an auth header to the forge origin.
pub fn auth_env(base_url: &str, header: &str) -> Vec<(String, String)> {
    let origin = match base_url.split_once("://") {
        Some((scheme, rest)) => format!("{scheme}://{}/", rest.split('/').next().unwrap_or("")),
        None => return Vec::new(),
    };
    vec![
        ("GIT_CONFIG_COUNT".into(), "1".into()),
        (
            "GIT_CONFIG_KEY_0".into(),
            format!("http.{origin}.extraHeader"),
        ),
        (
            "GIT_CONFIG_VALUE_0".into(),
            format!("Authorization: {header}"),
        ),
    ]
}

/// Clones `clone_url` into `dest` (no checkout: Warden only makes worktrees from it). An existing
/// clone at `dest` is reused. `clone_url` must be on `base_url`'s host.
pub async fn clone_managed(
    dest: &Path,
    clone_url: &str,
    base_url: &str,
    auth_header: Option<&str>,
) -> Result<()> {
    if dest.join(".git").is_dir() {
        return Ok(());
    }
    let (Some(url_host), Some(host)) = (
        normalize_remote(clone_url).map(|(h, _)| h),
        base_host(base_url),
    ) else {
        return Err(WardenError::Git(format!("bad clone URL {clone_url:?}")));
    };
    if url_host != host || !clone_url.starts_with("https://") && !clone_url.starts_with("http://") {
        return Err(WardenError::Git(format!(
            "refusing to clone {clone_url}: not an HTTPS URL on {host}"
        )));
    }
    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let partial = dest.with_extension("partial");
    let _ = tokio::fs::remove_dir_all(&partial).await;

    let mut cmd = Command::new("git");
    cmd.args([
        "clone",
        "--no-checkout",
        "--no-tags",
        "--end-of-options",
        clone_url,
    ])
    .arg(&partial)
    .env("GIT_TERMINAL_PROMPT", "0")
    .env("LC_ALL", "C")
    .stdin(Stdio::null())
    .stdout(Stdio::null())
    .stderr(Stdio::piped())
    .kill_on_drop(true);
    if let Some(h) = auth_header {
        for (k, v) in auth_env(base_url, h) {
            cmd.env(k, v);
        }
    }
    let child = cmd
        .spawn()
        .map_err(|e| WardenError::Git(format!("cannot run git: {e}")))?;
    let out = match tokio::time::timeout(CLONE_TIMEOUT, child.wait_with_output()).await {
        Ok(r) => r.map_err(|e| WardenError::Git(format!("git clone: {e}")))?,
        Err(_) => {
            let _ = tokio::fs::remove_dir_all(&partial).await;
            return Err(WardenError::Git("git clone timed out after 15 min".into()));
        }
    };
    if !out.status.success() {
        let _ = tokio::fs::remove_dir_all(&partial).await;
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(WardenError::Git(format!(
            "git clone failed: {}",
            err.trim().lines().last().unwrap_or("unknown error")
        )));
    }
    tokio::fs::rename(&partial, dest).await?;
    Ok(())
}

/// Target branches for a new project: the default branch, then protected branches, deduped.
pub fn target_branches(default_branch: Option<&str>, protected: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for b in default_branch
        .into_iter()
        .map(str::to_string)
        .chain(protected.iter().cloned())
    {
        let b = b.trim().to_string();
        if !b.is_empty() && !out.contains(&b) {
            out.push(b);
        }
    }
    if out.is_empty() {
        out = vec!["main".into(), "master".into(), "develop".into()];
    }
    out
}

/// Assembles a [`Discovery`].
pub fn discovery(
    base_url: &str,
    remote: Vec<RemoteProjectInfo>,
    roots: &[PathBuf],
    scanned: (Vec<ScannedRepo>, u32),
    existing: &[Project],
) -> Discovery {
    let (projects, local_repos) = merge(base_url, remote, &scanned.0, existing);
    Discovery {
        projects,
        local_repos,
        scanned_roots: roots
            .iter()
            .map(|r| r.to_string_lossy().to_string())
            .collect(),
        scanned_dirs: scanned.1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_remote_forms() {
        let want = Some(("gitlab.com".to_string(), "entries/entries_crm".to_string()));
        assert_eq!(
            normalize_remote("git@gitlab.com:entries/entries_crm.git"),
            want
        );
        assert_eq!(
            normalize_remote("ssh://git@gitlab.com:22/entries/entries_crm.git"),
            want
        );
        assert_eq!(
            normalize_remote("https://gitlab.com/entries/entries_crm"),
            want
        );
        assert_eq!(
            normalize_remote("https://oauth2:tok@GitLab.com/Entries/entries_crm.git/"),
            want
        );
        assert_eq!(normalize_remote("/Users/me/repo"), None);
        assert_eq!(normalize_remote(""), None);
    }

    #[test]
    fn remote_id_respects_host_and_subpath() {
        assert_eq!(
            remote_id_for("git@gitlab.com:a/b/c.git", "https://gitlab.com").as_deref(),
            Some("a/b/c")
        );
        assert_eq!(
            remote_id_for("git@github.com:a/b.git", "https://gitlab.com"),
            None
        );
        assert_eq!(
            remote_id_for(
                "https://git.acme.io/gitlab/team/app.git",
                "https://git.acme.io/gitlab"
            )
            .as_deref(),
            Some("team/app")
        );
    }

    #[test]
    fn parses_git_config_remotes() {
        let cfg = "[core]\n\tbare = false\n[remote \"origin\"]\n\turl = git@gitlab.com:e/crm.git\n\tfetch = +refs/heads/*:refs/remotes/origin/*\n[remote \"upstream\"]\n\turl = https://gitlab.com/e/crm\n[branch \"main\"]\n\tremote = origin\n";
        assert_eq!(
            parse_remotes(cfg),
            vec![
                ("origin".to_string(), "git@gitlab.com:e/crm.git".to_string()),
                (
                    "upstream".to_string(),
                    "https://gitlab.com/e/crm".to_string()
                )
            ]
        );
    }

    #[test]
    fn merge_matches_and_flags_duplicates() {
        let remote = vec![
            RemoteProjectInfo {
                remote_id: "e/ui".into(),
                name: "ui".into(),
                ..Default::default()
            },
            RemoteProjectInfo {
                remote_id: "e/crm".into(),
                name: "crm".into(),
                ..Default::default()
            },
        ];
        let scanned = vec![
            ScannedRepo {
                path: "/x/a/crm".into(),
                remotes: vec![("origin".into(), "git@gitlab.com:E/crm.git".into())],
            },
            ScannedRepo {
                path: "/x/b/crm".into(),
                remotes: vec![("origin".into(), "https://gitlab.com/e/crm".into())],
            },
            ScannedRepo {
                path: "/x/frappe".into(),
                remotes: vec![("upstream".into(), "https://github.com/frappe/frappe".into())],
            },
        ];
        let existing = vec![Project {
            id: 7,
            forge: ForgeKind::Gitlab,
            base_url: "https://gitlab.com/".into(),
            remote_id: "e/ui".into(),
            name: "ui".into(),
            local_path: String::new(),
            target_branches: vec![],
            mode: crate::types::PostingMode::Draft,
            poll_interval_secs: 300,
            enabled: true,
        }];
        let (projects, local) = merge("https://gitlab.com", remote, &scanned, &existing);
        assert_eq!(projects[0].remote.remote_id, "e/crm");
        assert_eq!(projects[0].local_paths, vec!["/x/a/crm", "/x/b/crm"]);
        assert_eq!(projects[1].existing_project_id, Some(7));
        assert_eq!(local[1].duplicate_of.as_deref(), Some("/x/a/crm"));
        assert_eq!(local[2].remote_id, None);
    }

    #[test]
    fn parses_mr_links() {
        let l = parse_mr_link(
            "https://gitlab.com/entries/crm/-/merge_requests/41/diffs?x=1",
            &[],
        )
        .unwrap();
        assert_eq!(
            (l.forge, l.base_url.as_str(), l.remote_id.as_str(), l.iid),
            (ForgeKind::Gitlab, "https://gitlab.com", "entries/crm", 41)
        );
        let bases = vec![(ForgeKind::Gitlab, "https://git.acme.io/gitlab".to_string())];
        let l = parse_mr_link(
            "https://git.acme.io/gitlab/team/app/-/merge_requests/3",
            &bases,
        )
        .unwrap();
        assert_eq!(
            (l.base_url.as_str(), l.remote_id.as_str()),
            ("https://git.acme.io/gitlab", "team/app")
        );
        let l = parse_mr_link("https://github.com/frappe/frappe/pull/123/files", &[]).unwrap();
        assert_eq!(
            (l.forge, l.remote_id.as_str(), l.iid),
            (ForgeKind::Github, "frappe/frappe", 123)
        );
        assert!(parse_mr_link("https://gitlab.com/entries/crm", &[]).is_none());
        assert!(parse_mr_link("file:///etc/passwd", &[]).is_none());
    }

    #[test]
    fn base64_and_auth_env() {
        assert_eq!(base64(b"oauth2:abc"), "b2F1dGgyOmFiYw==");
        assert_eq!(base64(b"ab"), "YWI=");
        assert_eq!(base64(b"abc"), "YWJj");
        let env = auth_env("https://git.acme.io/gitlab", "Basic xyz");
        assert_eq!(env[1].1, "http.https://git.acme.io/.extraHeader");
        assert_eq!(env[2].1, "Authorization: Basic xyz");
    }

    #[test]
    fn target_branches_dedupes() {
        assert_eq!(
            target_branches(Some("main"), &["main".into(), "release/*".into()]),
            vec!["main", "release/*"]
        );
        assert_eq!(
            target_branches(None, &[]),
            vec!["main", "master", "develop"]
        );
    }

    #[test]
    fn scan_finds_repos_and_skips_inside() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("bench/apps/crm");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::write(
            repo.join(".git/config"),
            "[remote \"origin\"]\n\turl = git@gitlab.com:e/crm.git\n",
        )
        .unwrap();
        std::fs::create_dir_all(repo.join("sub/.git")).unwrap();
        std::fs::create_dir_all(tmp.path().join("node_modules/x/.git")).unwrap();
        let (found, _) = scan_repos(&[tmp.path().to_path_buf()]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].path, repo);
    }

    #[test]
    fn clone_refuses_other_hosts() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let err = rt
            .block_on(clone_managed(
                &tmp.path().join("r"),
                "https://evil.example/x.git",
                "https://gitlab.com",
                None,
            ))
            .unwrap_err();
        assert!(err.to_string().contains("refusing"));
        let err = rt
            .block_on(clone_managed(
                &tmp.path().join("r"),
                "git@gitlab.com:x/y.git",
                "https://gitlab.com",
                None,
            ))
            .unwrap_err();
        assert!(err.to_string().contains("refusing"));
    }
}
