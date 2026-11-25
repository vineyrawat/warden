// OWNER: forge
//! octocrab-based GitHub client (read + review comment post).
//!
//! Uses octocrab's generic `get`/`post` with Warden's own lenient wire structs (octocrab's typed
//! models require dozens of fields, which makes them brittle against GHE versions and mocks).
//!
//! The octocrab client is built per call: `Octocrab::build()` spawns its buffer worker on the
//! *current* tokio runtime, so building it in `new()` would panic outside a runtime (e.g. in a sync
//! `provider_for`) and would break if the provider outlived that runtime. Retries are disabled so a
//! 5xx on a POST can never duplicate a comment.

use async_trait::async_trait;
use octocrab::service::middleware::retry::RetryConfig;
use octocrab::Octocrab;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use super::gitlab::encode_component;
use super::{truncate_msg, ForgeDiscussion, ForgeNote, ForgeProvider};
use crate::error::{Result, WardenError};
use crate::types::{DiffPosition, MergeRequest, Project, RemoteProjectInfo};

const PER_PAGE: u32 = 100;
const MAX_PAGES: u32 = 50;

pub struct GithubProvider {
    /// Web base URL as configured (e.g. `https://github.com`).
    pub base_url: String,
    pub token: String,
    /// REST API root derived from `base_url` (see [`api_base_for`]).
    api_base: String,
}

impl GithubProvider {
    pub fn new(base_url: impl Into<String>, token: impl Into<String>) -> Self {
        let base_url = base_url.into().trim().trim_end_matches('/').to_string();
        GithubProvider {
            api_base: api_base_for(&base_url),
            base_url,
            token: token.into(),
        }
    }

    /// Uses `api_base` verbatim as the REST root (tests point this at a wiremock server).
    pub fn with_api_base(api_base: impl Into<String>, token: impl Into<String>) -> Self {
        let api_base = api_base.into().trim().trim_end_matches('/').to_string();
        GithubProvider {
            base_url: api_base.clone(),
            api_base,
            token: token.into(),
        }
    }

    pub fn api_base(&self) -> &str {
        &self.api_base
    }

    fn client(&self) -> Result<Octocrab> {
        Octocrab::builder()
            .add_retry_config(RetryConfig::None)
            .base_uri(self.api_base.as_str())
            .map_err(|e| {
                WardenError::Config(format!("invalid GitHub API URL {}: {e}", self.api_base))
            })?
            .personal_token(self.token.clone())
            .build()
            .map_err(map_err)
    }

    async fn get<T: DeserializeOwned>(
        &self,
        route: &str,
        query: Option<&[(&str, String)]>,
    ) -> Result<T> {
        let client = self.client()?;
        let v: serde_json::Value = client.get(route, query).await.map_err(map_err)?;
        decode(v)
    }

    async fn post<B: Serialize + ?Sized, T: DeserializeOwned>(
        &self,
        route: &str,
        body: &B,
    ) -> Result<T> {
        let client = self.client()?;
        let v: serde_json::Value = client.post(route, Some(body)).await.map_err(map_err)?;
        decode(v)
    }

    /// Page-number pagination: stops at the first short page.
    async fn get_all<T: DeserializeOwned>(
        &self,
        route: &str,
        query: &[(&str, String)],
    ) -> Result<Vec<T>> {
        let mut out = Vec::new();
        for page in 1..=MAX_PAGES {
            let mut q = query.to_vec();
            q.push(("per_page", PER_PAGE.to_string()));
            q.push(("page", page.to_string()));
            let items: Vec<T> = self.get(route, Some(&q)).await?;
            let n = items.len();
            out.extend(items);
            if n < PER_PAGE as usize {
                return Ok(out);
            }
        }
        tracing::warn!(route, "github pagination stopped after {MAX_PAGES} pages");
        Ok(out)
    }

    fn repo_route(project: &Project) -> Result<String> {
        let (owner, repo) = split_repo(&project.remote_id)?;
        Ok(format!(
            "/repos/{}/{}",
            encode_component(owner),
            encode_component(repo)
        ))
    }
}

#[async_trait]
impl ForgeProvider for GithubProvider {
    async fn whoami(&self) -> Result<String> {
        let user: GhUser = self.get("/user", None).await?;
        Ok(user.login)
    }

    async fn list_projects(&self, search: &str) -> Result<Vec<(String, String)>> {
        let repos: Vec<GhRepo> = self
            .get_all(
                "/user/repos",
                &[
                    ("sort", "pushed".to_string()),
                    (
                        "affiliation",
                        "owner,collaborator,organization_member".to_string(),
                    ),
                ],
            )
            .await?;
        let needle = search.trim().to_ascii_lowercase();
        Ok(repos
            .into_iter()
            .filter(|r| needle.is_empty() || r.full_name.to_ascii_lowercase().contains(&needle))
            .map(|r| {
                let name = r.name.unwrap_or_else(|| r.full_name.clone());
                (r.full_name, name)
            })
            .collect())
    }

    async fn list_project_details(&self, search: &str) -> Result<Vec<RemoteProjectInfo>> {
        let repos: Vec<GhRepo> = self
            .get_all(
                "/user/repos",
                &[
                    ("sort", "pushed".to_string()),
                    (
                        "affiliation",
                        "owner,collaborator,organization_member".to_string(),
                    ),
                ],
            )
            .await?;
        let needle = search.trim().to_ascii_lowercase();
        Ok(repos
            .into_iter()
            .filter(|r| !r.archived)
            .filter(|r| needle.is_empty() || r.full_name.to_ascii_lowercase().contains(&needle))
            .map(|r| RemoteProjectInfo {
                name: r.name.clone().unwrap_or_else(|| r.full_name.clone()),
                remote_id: r.full_name,
                default_branch: r.default_branch,
                last_activity_at: r.pushed_at,
                web_url: r.html_url,
                clone_url: r.clone_url,
            })
            .collect())
    }

    async fn protected_branches(&self, remote_id: &str) -> Result<Vec<String>> {
        let (owner, repo) = split_repo(remote_id)?;
        let route = format!(
            "/repos/{}/{}/branches",
            encode_component(owner),
            encode_component(repo)
        );
        let raw: Vec<GhBranchName> = self
            .get_all(&route, &[("protected", "true".to_string())])
            .await?;
        Ok(raw
            .into_iter()
            .map(|b| b.name)
            .filter(|n| !n.is_empty())
            .collect())
    }

    fn git_auth_header(&self) -> Option<String> {
        Some(crate::discovery::basic_auth_header(
            "x-access-token",
            &self.token,
        ))
    }

    async fn list_open_mrs(&self, project: &Project) -> Result<Vec<MergeRequest>> {
        let route = format!("{}/pulls", Self::repo_route(project)?);
        let prs: Vec<GhPull> = self
            .get_all(
                &route,
                &[
                    ("state", "open".to_string()),
                    ("sort", "updated".to_string()),
                    ("direction", "desc".to_string()),
                ],
            )
            .await?;
        let mut mrs: Vec<MergeRequest> =
            prs.into_iter().map(|p| p.into_model(project.id)).collect();
        mrs.sort_by_key(|m| m.iid);
        Ok(mrs)
    }

    async fn get_mr(&self, project: &Project, iid: u64) -> Result<MergeRequest> {
        let route = format!("{}/pulls/{iid}", Self::repo_route(project)?);
        let pr: GhPull = self.get(&route, None).await?;
        Ok(pr.into_model(project.id))
    }

    /// Review comments grouped into threads (root comment id = discussion id), followed by
    /// top-level PR conversation comments as one-note discussions with id `issuecomment-<id>`.
    /// GitHub's REST API does not expose thread resolution, so `resolved` is always false.
    async fn list_discussions(&self, project: &Project, iid: u64) -> Result<Vec<ForgeDiscussion>> {
        let repo = Self::repo_route(project)?;
        let review: Vec<GhReviewComment> = self
            .get_all(&format!("{repo}/pulls/{iid}/comments"), &[])
            .await?;
        let issue: Vec<GhIssueComment> = self
            .get_all(&format!("{repo}/issues/{iid}/comments"), &[])
            .await?;

        let mut threads: Vec<ForgeDiscussion> = Vec::new();
        let mut index: std::collections::HashMap<u64, usize> = std::collections::HashMap::new();
        let mut review = review;
        review.sort_by_key(|c| c.id);
        for c in review {
            let note = ForgeNote {
                id: c.id.to_string(),
                author: c.user.map(|u| u.login).unwrap_or_default(),
                body: c.body.clone(),
                path: c.path.clone(),
                line: c.line.or(c.original_line),
            };
            let root = c.in_reply_to_id.unwrap_or(c.id);
            match index.get(&root) {
                Some(&i) => threads[i].notes.push(note),
                None => {
                    index.insert(root, threads.len());
                    threads.push(ForgeDiscussion {
                        id: root.to_string(),
                        resolved: false,
                        notes: vec![note],
                    });
                }
            }
        }
        for c in issue {
            threads.push(ForgeDiscussion {
                id: format!("issuecomment-{}", c.id),
                resolved: false,
                notes: vec![ForgeNote {
                    id: c.id.to_string(),
                    author: c.user.map(|u| u.login).unwrap_or_default(),
                    body: c.body,
                    path: None,
                    line: None,
                }],
            });
        }
        Ok(threads)
    }

    /// `POST /repos/:o/:r/pulls/:n/comments` with `commit_id` + `path` + `line` + `side`.
    /// GitHub answers 422 when the line is not part of the diff; that is reported as
    /// `Forge { status: 400 }` so the poster's position fallback works for both forges.
    async fn post_line_note(
        &self,
        project: &Project,
        iid: u64,
        body: &str,
        pos: &DiffPosition,
    ) -> Result<String> {
        let route = format!("{}/pulls/{iid}/comments", Self::repo_route(project)?);
        let req = review_comment_request(body, pos)?;
        match self.post::<_, GhCreated>(&route, &req).await {
            Ok(c) => Ok(c.id.to_string()),
            Err(WardenError::Forge { status: 422, msg }) => Err(WardenError::Forge {
                status: 400,
                msg: format!("github 422 (position rejected): {msg}"),
            }),
            Err(e) => Err(e),
        }
    }

    async fn post_note(&self, project: &Project, iid: u64, body: &str) -> Result<String> {
        let route = format!("{}/issues/{iid}/comments", Self::repo_route(project)?);
        let c: GhCreated = self
            .post(&route, &serde_json::json!({ "body": body }))
            .await?;
        Ok(c.id.to_string())
    }

    async fn resolve_discussion(
        &self,
        project: &Project,
        iid: u64,
        discussion_id: &str,
    ) -> Result<()> {
        tracing::warn!(
            repo = %project.remote_id,
            pr = iid,
            discussion_id,
            "GitHub resolves review threads only via GraphQL; resolve_discussion is a no-op"
        );
        Ok(())
    }

    async fn add_label(&self, project: &Project, iid: u64, label: &str) -> Result<()> {
        let route = format!("{}/issues/{iid}/labels", Self::repo_route(project)?);
        let _: serde_json::Value = self
            .post(&route, &serde_json::json!({ "labels": [label] }))
            .await?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------------------------

/// REST root for a GitHub web URL: github.com -> `https://api.github.com`; a URL that already
/// points at an API (`api.` host or `/api/v3` path) is used as-is; any other host is GitHub
/// Enterprise Server -> `<base>/api/v3`.
pub fn api_base_for(base_url: &str) -> String {
    let b = base_url.trim().trim_end_matches('/');
    let lower = b.to_ascii_lowercase();
    let host = lower
        .split("://")
        .nth(1)
        .unwrap_or(&lower)
        .split('/')
        .next()
        .unwrap_or("");
    if host.is_empty() || host == "github.com" || host == "www.github.com" {
        "https://api.github.com".to_string()
    } else if host.starts_with("api.") || lower.ends_with("/api/v3") {
        b.to_string()
    } else {
        format!("{b}/api/v3")
    }
}

fn split_repo(remote_id: &str) -> Result<(&str, &str)> {
    let id = remote_id.trim().trim_matches('/');
    match id.split_once('/') {
        Some((o, r)) if !o.is_empty() && !r.is_empty() && !r.contains('/') => Ok((o, r)),
        _ => Err(WardenError::Config(format!(
            "GitHub remote_id must be \"owner/repo\", got {remote_id:?}"
        ))),
    }
}

#[derive(Debug, Serialize, PartialEq)]
struct ReviewCommentRequest {
    body: String,
    commit_id: String,
    path: String,
    line: u32,
    side: &'static str,
}

fn review_comment_request(body: &str, pos: &DiffPosition) -> Result<ReviewCommentRequest> {
    let (path, line, side) = match (pos.new_line, pos.old_line) {
        (Some(n), _) => (&pos.new_path, n, "RIGHT"),
        (None, Some(o)) => (&pos.old_path, o, "LEFT"),
        (None, None) => {
            return Err(WardenError::Forge {
                status: 400,
                msg: "position has neither new_line nor old_line".into(),
            })
        }
    };
    Ok(ReviewCommentRequest {
        body: body.to_string(),
        commit_id: pos.head_sha.clone(),
        path: path.clone(),
        line,
        side,
    })
}

fn decode<T: DeserializeOwned>(v: serde_json::Value) -> Result<T> {
    let preview = truncate_msg(&v.to_string());
    serde_json::from_value(v).map_err(|e| {
        WardenError::Http(format!(
            "github returned unexpected JSON: {e}; body: {preview}"
        ))
    })
}

/// octocrab errors -> WardenError: GitHub API errors keep their HTTP status.
fn map_err(e: octocrab::Error) -> WardenError {
    match e {
        octocrab::Error::GitHub { source, .. } => {
            let status = source.status_code.as_u16();
            let mut msg = source.message.clone();
            if let Some(errs) = &source.errors {
                if !errs.is_empty() {
                    msg = format!("{msg}: {}", serde_json::Value::Array(errs.clone()));
                }
            }
            let msg = match status {
                401 => format!("unauthorized (check the GitHub token): {msg}"),
                403 => format!("forbidden (token scope or rate limit): {msg}"),
                _ => msg,
            };
            WardenError::Forge {
                status,
                msg: truncate_msg(&msg),
            }
        }
        other => WardenError::Http(format!("github request failed: {other}")),
    }
}

// ---------------------------------------------------------------------------------------------
// Wire types
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct GhUser {
    login: String,
}

#[derive(Debug, Deserialize)]
struct GhRepo {
    full_name: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    default_branch: Option<String>,
    #[serde(default)]
    pushed_at: Option<String>,
    #[serde(default)]
    html_url: Option<String>,
    #[serde(default)]
    clone_url: Option<String>,
    #[serde(default)]
    archived: bool,
}

#[derive(Debug, Default, Deserialize)]
struct GhBranchName {
    #[serde(default)]
    name: String,
}

#[derive(Debug, Default, Deserialize)]
struct GhRef {
    #[serde(rename = "ref", default)]
    ref_: String,
    #[serde(default)]
    sha: String,
}

#[derive(Debug, Deserialize)]
struct GhLabel {
    name: String,
}

#[derive(Debug, Deserialize)]
struct GhPull {
    number: u64,
    #[serde(default)]
    title: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    user: Option<GhUser>,
    #[serde(default)]
    head: GhRef,
    #[serde(default)]
    base: GhRef,
    #[serde(default)]
    html_url: String,
    #[serde(default)]
    draft: Option<bool>,
    #[serde(default)]
    labels: Vec<GhLabel>,
    #[serde(default)]
    state: String,
    #[serde(default)]
    merged_at: Option<String>,
    #[serde(default)]
    merged: Option<bool>,
    #[serde(default)]
    updated_at: Option<String>,
    /// Only on the single-PR endpoint.
    #[serde(default)]
    additions: Option<u32>,
    #[serde(default)]
    deletions: Option<u32>,
}

impl GhPull {
    fn into_model(self, project_id: i64) -> MergeRequest {
        let state = match self.state.as_str() {
            "open" => "opened".to_string(),
            "closed" if self.merged_at.is_some() || self.merged == Some(true) => {
                "merged".to_string()
            }
            "closed" => "closed".to_string(),
            other => other.to_string(),
        };
        MergeRequest {
            id: 0,
            project_id,
            iid: self.number,
            title: self.title,
            description: self.body.unwrap_or_default(),
            author: self.user.map(|u| u.login).unwrap_or_default(),
            source_branch: self.head.ref_,
            target_branch: self.base.ref_,
            web_url: self.html_url,
            draft: self.draft.unwrap_or(false),
            labels: self.labels.into_iter().map(|l| l.name).collect(),
            head_sha: self.head.sha,
            // GitHub has no separate start sha; base.sha is the target tip the PR was compared to.
            start_sha: self.base.sha.clone(),
            base_sha: self.base.sha,
            last_reviewed_sha: None,
            additions: self.additions.unwrap_or(0),
            deletions: self.deletions.unwrap_or(0),
            state,
            updated_at: self.updated_at.unwrap_or_default(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct GhReviewComment {
    id: u64,
    #[serde(default)]
    body: String,
    #[serde(default)]
    user: Option<GhUser>,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    line: Option<u32>,
    #[serde(default)]
    original_line: Option<u32>,
    #[serde(default)]
    in_reply_to_id: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct GhIssueComment {
    id: u64,
    #[serde(default)]
    body: String,
    #[serde(default)]
    user: Option<GhUser>,
}

#[derive(Debug, Deserialize)]
struct GhCreated {
    id: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_base_mapping() {
        assert_eq!(api_base_for("https://github.com"), "https://api.github.com");
        assert_eq!(
            api_base_for("https://github.com/"),
            "https://api.github.com"
        );
        assert_eq!(
            api_base_for("https://api.github.com"),
            "https://api.github.com"
        );
        assert_eq!(
            api_base_for("https://ghe.corp.io"),
            "https://ghe.corp.io/api/v3"
        );
        assert_eq!(
            api_base_for("https://ghe.corp.io/api/v3/"),
            "https://ghe.corp.io/api/v3"
        );
        assert_eq!(
            api_base_for("http://127.0.0.1:8080"),
            "http://127.0.0.1:8080/api/v3"
        );
    }

    #[test]
    fn repo_split() {
        assert_eq!(split_repo("o/r").unwrap(), ("o", "r"));
        assert_eq!(split_repo("/o/r/").unwrap(), ("o", "r"));
        assert!(split_repo("o").is_err());
        assert!(split_repo("o/r/x").is_err());
        assert!(split_repo("/r").is_err());
    }

    #[test]
    fn review_comment_sides() {
        let mut pos = DiffPosition {
            base_sha: "b".into(),
            start_sha: "s".into(),
            head_sha: "h".into(),
            old_path: "old.py".into(),
            new_path: "new.py".into(),
            old_line: Some(3),
            new_line: Some(5),
        };
        let r = review_comment_request("x", &pos).unwrap();
        assert_eq!(
            (r.path.as_str(), r.line, r.side, r.commit_id.as_str()),
            ("new.py", 5, "RIGHT", "h")
        );
        pos.new_line = None;
        let r = review_comment_request("x", &pos).unwrap();
        assert_eq!((r.path.as_str(), r.line, r.side), ("old.py", 3, "LEFT"));
        pos.old_line = None;
        assert!(matches!(
            review_comment_request("x", &pos),
            Err(WardenError::Forge { status: 400, .. })
        ));
    }

    #[test]
    fn pull_state_mapping() {
        let pr: GhPull = serde_json::from_value(serde_json::json!({
            "number": 3, "state": "closed", "merged_at": "2026-01-01T00:00:00Z",
            "head": {"ref": "f", "sha": "h"}, "base": {"ref": "main", "sha": "b"}
        }))
        .unwrap();
        let mr = pr.into_model(2);
        assert_eq!(mr.state, "merged");
        assert_eq!(mr.start_sha, "b");
        assert_eq!(mr.base_sha, "b");
        assert_eq!(mr.head_sha, "h");
    }
}
