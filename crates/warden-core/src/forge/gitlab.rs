// OWNER: forge
//! reqwest GitLab v4 client. Header PRIVATE-TOKEN; project id = URL-encoded path.
//! Endpoints: /api/v4/projects/:id/merge_requests?state=opened, /merge_requests/:iid (diff_refs),
//! /merge_requests/:iid/discussions (paginate via x-next-page), POST discussions with
//! position[position_type]=text, PUT discussions/:id?resolved=true, PUT merge_requests/:iid add_labels.

use std::time::Duration;

use async_trait::async_trait;
use futures::stream::{self, StreamExt, TryStreamExt};
use reqwest::{Method, RequestBuilder, Response};
use serde::de::DeserializeOwned;
use serde::Deserialize;

use super::{truncate_msg, ForgeDiscussion, ForgeNote, ForgeProvider};
use crate::error::{Result, WardenError};
use crate::types::{DiffPosition, MergeRequest, Project, RemoteProjectInfo};

/// Items per page for list endpoints (GitLab max is 100).
const PER_PAGE: u32 = 100;
/// Hard stop for pagination loops (100 pages * 100 items).
const MAX_PAGES: u32 = 100;
/// Concurrent `GET merge_requests/:iid` calls when the list endpoint lacks diff_refs.
const DETAIL_CONCURRENCY: usize = 4;

pub struct GitlabProvider {
    pub base_url: String,
    pub token: String,
    client: reqwest::Client,
}

impl GitlabProvider {
    pub fn new(base_url: impl Into<String>, token: impl Into<String>) -> Self {
        let client = reqwest::Client::builder()
            .user_agent(concat!("warden/", env!("CARGO_PKG_VERSION")))
            .timeout(Duration::from_secs(30))
            .connect_timeout(Duration::from_secs(10))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        GitlabProvider {
            base_url: normalize_base(&base_url.into()),
            token: token.into(),
            client,
        }
    }

    /// `https://gitlab.example.com/api/v4` + `path` (path starts with `/`).
    fn url(&self, path: &str) -> String {
        format!("{}/api/v4{}", self.base_url, path)
    }

    fn project_path(project: &Project) -> String {
        format!("/projects/{}", encode_project_id(&project.remote_id))
    }

    fn mr_path(project: &Project, iid: u64) -> String {
        format!("{}/merge_requests/{}", Self::project_path(project), iid)
    }

    fn request(&self, method: Method, path: &str) -> RequestBuilder {
        self.client
            .request(method, self.url(path))
            .header("PRIVATE-TOKEN", &self.token)
            .header(reqwest::header::ACCEPT, "application/json")
    }

    async fn send(&self, rb: RequestBuilder) -> Result<Response> {
        let resp = rb.send().await.map_err(|e| {
            // reqwest errors may embed the URL; never the token (it is a header).
            WardenError::Http(format!("gitlab request failed: {e}"))
        })?;
        check_status(resp).await
    }

    async fn json<T: DeserializeOwned>(&self, rb: RequestBuilder) -> Result<T> {
        let resp = self.send(rb).await?;
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| WardenError::Http(format!("gitlab response read failed: {e}")))?;
        serde_json::from_slice(&bytes).map_err(|e| {
            WardenError::Http(format!(
                "gitlab returned unexpected JSON: {e}; body: {}",
                truncate_msg(&String::from_utf8_lossy(&bytes))
            ))
        })
    }

    /// GET with pagination: follows `x-next-page` (falls back to `x-total-pages`).
    async fn get_paginated<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<Vec<T>> {
        let mut out = Vec::new();
        let mut page: u32 = 1;
        for _ in 0..MAX_PAGES {
            let mut q: Vec<(&str, String)> = query.to_vec();
            q.push(("per_page", PER_PAGE.to_string()));
            q.push(("page", page.to_string()));
            let resp = self.send(self.request(Method::GET, path).query(&q)).await?;
            let next = next_page(resp.headers(), page);
            let bytes = resp
                .bytes()
                .await
                .map_err(|e| WardenError::Http(format!("gitlab response read failed: {e}")))?;
            let items: Vec<T> = serde_json::from_slice(&bytes).map_err(|e| {
                WardenError::Http(format!(
                    "gitlab returned unexpected JSON: {e}; body: {}",
                    truncate_msg(&String::from_utf8_lossy(&bytes))
                ))
            })?;
            let empty = items.is_empty();
            out.extend(items);
            match next {
                Some(n) if n > page && !empty => page = n,
                _ => return Ok(out),
            }
        }
        tracing::warn!(path, "gitlab pagination stopped after {MAX_PAGES} pages");
        Ok(out)
    }
}

#[async_trait]
impl ForgeProvider for GitlabProvider {
    async fn whoami(&self) -> Result<String> {
        let user: GlUser = self.json(self.request(Method::GET, "/user")).await?;
        Ok(user.username)
    }

    async fn list_projects(&self, search: &str) -> Result<Vec<(String, String)>> {
        let mut q = vec![
            ("membership", "true".to_string()),
            ("simple", "true".to_string()),
            ("order_by", "last_activity_at".to_string()),
        ];
        let search = search.trim();
        if !search.is_empty() {
            q.push(("search", search.to_string()));
            // Lets "group/repo" queries match the full path, not just the name.
            q.push(("search_namespaces", "true".to_string()));
        }
        let projects: Vec<GlProject> = self.get_paginated("/projects", &q).await?;
        Ok(projects
            .into_iter()
            .map(|p| {
                let name = p
                    .name_with_namespace
                    .or(p.name)
                    .unwrap_or_else(|| p.path_with_namespace.clone());
                (p.path_with_namespace, name)
            })
            .collect())
    }

    async fn list_project_details(&self, search: &str) -> Result<Vec<RemoteProjectInfo>> {
        let mut q = vec![
            ("membership", "true".to_string()),
            ("simple", "true".to_string()),
            ("archived", "false".to_string()),
            ("order_by", "last_activity_at".to_string()),
        ];
        let search = search.trim();
        if !search.is_empty() {
            q.push(("search", search.to_string()));
            q.push(("search_namespaces", "true".to_string()));
        }
        let projects: Vec<GlProject> = self.get_paginated("/projects", &q).await?;
        Ok(projects
            .into_iter()
            .map(|p| RemoteProjectInfo {
                name: p.name.clone().unwrap_or_else(|| {
                    p.path_with_namespace
                        .rsplit('/')
                        .next()
                        .unwrap_or("")
                        .to_string()
                }),
                remote_id: p.path_with_namespace,
                default_branch: p.default_branch,
                last_activity_at: p.last_activity_at,
                web_url: p.web_url,
                clone_url: p.http_url_to_repo,
            })
            .collect())
    }

    async fn protected_branches(&self, remote_id: &str) -> Result<Vec<String>> {
        let path = format!(
            "/projects/{}/protected_branches",
            encode_project_id(remote_id)
        );
        let raw: Vec<GlBranchName> = self.get_paginated(&path, &[]).await?;
        Ok(raw
            .into_iter()
            .map(|b| b.name)
            .filter(|n| !n.is_empty())
            .collect())
    }

    fn git_auth_header(&self) -> Option<String> {
        Some(crate::discovery::basic_auth_header("oauth2", &self.token))
    }

    async fn list_open_mrs(&self, project: &Project) -> Result<Vec<MergeRequest>> {
        let path = format!("{}/merge_requests", Self::project_path(project));
        let raw: Vec<GlMergeRequest> = self
            .get_paginated(&path, &[("state", "opened".to_string())])
            .await?;

        // The list endpoint has no diff_refs; fetch the detail view for those (bounded concurrency).
        let mut mrs: Vec<MergeRequest> = stream::iter(raw.into_iter().map(|r| async move {
            if r.diff_refs.as_ref().is_some_and(DiffRefs::complete) {
                Ok(r.into_model(project.id))
            } else {
                self.get_mr(project, r.iid).await
            }
        }))
        .buffer_unordered(DETAIL_CONCURRENCY)
        .try_collect()
        .await?;
        mrs.sort_by_key(|m| m.iid);
        Ok(mrs)
    }

    async fn get_mr(&self, project: &Project, iid: u64) -> Result<MergeRequest> {
        let raw: GlMergeRequest = self
            .json(self.request(Method::GET, &Self::mr_path(project, iid)))
            .await?;
        Ok(raw.into_model(project.id))
    }

    async fn list_discussions(&self, project: &Project, iid: u64) -> Result<Vec<ForgeDiscussion>> {
        let path = format!("{}/discussions", Self::mr_path(project, iid));
        let raw: Vec<GlDiscussion> = self.get_paginated(&path, &[]).await?;
        Ok(raw
            .into_iter()
            .filter_map(GlDiscussion::into_model)
            .collect())
    }

    async fn post_line_note(
        &self,
        project: &Project,
        iid: u64,
        body: &str,
        pos: &DiffPosition,
    ) -> Result<String> {
        let path = format!("{}/discussions", Self::mr_path(project, iid));
        let form = line_note_form(body, pos);
        let created: GlDiscussionCreated = self
            .json(self.request(Method::POST, &path).form(&form))
            .await?;
        Ok(created.id)
    }

    async fn post_note(&self, project: &Project, iid: u64, body: &str) -> Result<String> {
        let path = format!("{}/notes", Self::mr_path(project, iid));
        let created: GlNoteCreated = self
            .json(self.request(Method::POST, &path).form(&[("body", body)]))
            .await?;
        Ok(created.id.to_string())
    }

    /// `POST /merge_requests/:iid/discussions` with only a body: a resolvable thread (a plain
    /// `/notes` note has a numeric id that `PUT /discussions/:id` cannot resolve).
    async fn post_file_note(
        &self,
        project: &Project,
        iid: u64,
        body: &str,
    ) -> Result<(String, bool)> {
        let path = format!("{}/discussions", Self::mr_path(project, iid));
        let created: GlDiscussionCreated = self
            .json(self.request(Method::POST, &path).form(&[("body", body)]))
            .await?;
        Ok((created.id, true))
    }

    async fn resolve_discussion(
        &self,
        project: &Project,
        iid: u64,
        discussion_id: &str,
    ) -> Result<()> {
        let path = format!(
            "{}/discussions/{}",
            Self::mr_path(project, iid),
            encode_component(discussion_id)
        );
        self.send(
            self.request(Method::PUT, &path)
                .query(&[("resolved", "true")]),
        )
        .await?;
        Ok(())
    }

    async fn add_label(&self, project: &Project, iid: u64, label: &str) -> Result<()> {
        self.send(
            self.request(Method::PUT, &Self::mr_path(project, iid))
                .form(&[("add_labels", label)]),
        )
        .await?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------------------------

/// Trims trailing slashes and a trailing `/api/v4` so both `https://gitlab.com` and
/// `https://gitlab.com/api/v4/` work.
fn normalize_base(base: &str) -> String {
    let mut b = base.trim().trim_end_matches('/').to_string();
    if let Some(stripped) = b.strip_suffix("/api/v4") {
        b = stripped.trim_end_matches('/').to_string();
    }
    b
}

/// Numeric ids pass through; paths are percent-encoded (`entries/entries_crm` -> `entries%2Fentries_crm`).
pub fn encode_project_id(remote_id: &str) -> String {
    let id = remote_id.trim().trim_matches('/');
    if !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()) {
        id.to_string()
    } else {
        encode_component(id)
    }
}

/// RFC 3986 path-segment encoding: everything except unreserved characters is percent-encoded.
pub(crate) fn encode_component(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Next page number from GitLab pagination headers. `x-next-page` is empty on the last page;
/// when it is absent (some proxies strip it, keyset pagination) fall back to `x-total-pages`.
fn next_page(headers: &reqwest::header::HeaderMap, current: u32) -> Option<u32> {
    let header = |name: &str| {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.trim().to_string())
    };
    match header("x-next-page") {
        Some(v) if v.is_empty() => None,
        Some(v) => v.parse().ok(),
        None => {
            let total: u32 = header("x-total-pages")?.parse().ok()?;
            (current < total).then_some(current + 1)
        }
    }
}

/// Maps a non-2xx response to `WardenError::Forge { status, msg }`.
async fn check_status(resp: Response) -> Result<Response> {
    let status = resp.status();
    if status.is_success() {
        return Ok(resp);
    }
    let code = status.as_u16();
    let body = resp.text().await.unwrap_or_default();
    let detail = error_message(&body);
    let msg = match code {
        401 => format!("unauthorized (check the GitLab token): {detail}"),
        403 => format!("forbidden (token lacks the api scope or project access): {detail}"),
        404 => format!("not found: {detail}"),
        _ => detail,
    };
    Err(WardenError::Forge {
        status: code,
        msg: truncate_msg(&msg),
    })
}

/// GitLab error bodies: `{"message": "..."}`, `{"message": {"field": ["..."]}}`, `{"error": "..."}`.
fn error_message(body: &str) -> String {
    match serde_json::from_str::<serde_json::Value>(body) {
        Ok(v) => {
            let pick = v
                .get("message")
                .or_else(|| v.get("error_description"))
                .or_else(|| v.get("error"));
            match pick {
                Some(serde_json::Value::String(s)) => s.clone(),
                Some(other) => other.to_string(),
                None => v.to_string(),
            }
        }
        Err(_) if body.trim().is_empty() => "empty response body".to_string(),
        Err(_) => body.to_string(),
    }
}

/// Form fields for `POST .../discussions` with a text position.
fn line_note_form(body: &str, pos: &DiffPosition) -> Vec<(&'static str, String)> {
    let mut form = vec![
        ("body", body.to_string()),
        ("position[position_type]", "text".to_string()),
        ("position[base_sha]", pos.base_sha.clone()),
        ("position[start_sha]", pos.start_sha.clone()),
        ("position[head_sha]", pos.head_sha.clone()),
        ("position[old_path]", pos.old_path.clone()),
        ("position[new_path]", pos.new_path.clone()),
    ];
    if let Some(n) = pos.new_line {
        form.push(("position[new_line]", n.to_string()));
    }
    if let Some(o) = pos.old_line {
        form.push(("position[old_line]", o.to_string()));
    }
    form
}

// ---------------------------------------------------------------------------------------------
// Wire types (lenient: GitLab versions differ; everything optional has a default)
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct GlUser {
    username: String,
}

#[derive(Debug, Deserialize)]
struct GlProject {
    path_with_namespace: String,
    #[serde(default)]
    name_with_namespace: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    default_branch: Option<String>,
    #[serde(default)]
    last_activity_at: Option<String>,
    #[serde(default)]
    web_url: Option<String>,
    #[serde(default)]
    http_url_to_repo: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct GlBranchName {
    #[serde(default)]
    name: String,
}

#[derive(Debug, Default, Deserialize)]
struct GlAuthor {
    #[serde(default)]
    username: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct DiffRefs {
    #[serde(default)]
    base_sha: Option<String>,
    #[serde(default)]
    start_sha: Option<String>,
    #[serde(default)]
    head_sha: Option<String>,
}

impl DiffRefs {
    fn complete(&self) -> bool {
        [&self.base_sha, &self.start_sha, &self.head_sha]
            .iter()
            .all(|s| s.as_deref().is_some_and(|v| !v.is_empty()))
    }
}

#[derive(Debug, Deserialize)]
struct GlMergeRequest {
    iid: u64,
    #[serde(default)]
    title: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    author: Option<GlAuthor>,
    #[serde(default)]
    source_branch: String,
    #[serde(default)]
    target_branch: String,
    #[serde(default)]
    web_url: String,
    #[serde(default)]
    draft: Option<bool>,
    #[serde(default)]
    work_in_progress: Option<bool>,
    /// Strings, or objects with `name` when `with_labels_details=true`.
    #[serde(default)]
    labels: Vec<serde_json::Value>,
    #[serde(default)]
    sha: Option<String>,
    #[serde(default)]
    diff_refs: Option<DiffRefs>,
    #[serde(default)]
    state: String,
    #[serde(default)]
    updated_at: Option<String>,
    /// Not in stock GitLab responses, but honoured when present (some proxies/versions add them).
    #[serde(default)]
    additions: Option<u32>,
    #[serde(default)]
    deletions: Option<u32>,
}

impl GlMergeRequest {
    fn into_model(self, project_id: i64) -> MergeRequest {
        let refs = self.diff_refs.unwrap_or_default();
        let head_sha = refs
            .head_sha
            .filter(|s| !s.is_empty())
            .or(self.sha)
            .unwrap_or_default();
        let labels = self
            .labels
            .into_iter()
            .filter_map(|l| match l {
                serde_json::Value::String(s) => Some(s),
                serde_json::Value::Object(o) => {
                    o.get("name").and_then(|n| n.as_str()).map(str::to_string)
                }
                _ => None,
            })
            .collect();
        MergeRequest {
            id: 0,
            project_id,
            iid: self.iid,
            title: self.title,
            description: self.description.unwrap_or_default(),
            author: self.author.map(|a| a.username).unwrap_or_default(),
            source_branch: self.source_branch,
            target_branch: self.target_branch,
            web_url: self.web_url,
            draft: self.draft.unwrap_or(false) || self.work_in_progress.unwrap_or(false),
            labels,
            head_sha,
            base_sha: refs.base_sha.unwrap_or_default(),
            start_sha: refs.start_sha.unwrap_or_default(),
            last_reviewed_sha: None,
            additions: self.additions.unwrap_or(0),
            deletions: self.deletions.unwrap_or(0),
            state: self.state,
            updated_at: self.updated_at.unwrap_or_default(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct GlDiscussion {
    id: String,
    #[serde(default)]
    notes: Vec<GlNote>,
}

impl GlDiscussion {
    /// Drops system notes ("added 2 commits", ...); discussions with nothing left are skipped.
    fn into_model(self) -> Option<ForgeDiscussion> {
        let notes: Vec<GlNote> = self.notes.into_iter().filter(|n| !n.system).collect();
        if notes.is_empty() {
            return None;
        }
        let resolvable: Vec<&GlNote> = notes.iter().filter(|n| n.resolvable).collect();
        let resolved = !resolvable.is_empty() && resolvable.iter().all(|n| n.resolved);
        Some(ForgeDiscussion {
            id: self.id,
            resolved,
            notes: notes
                .into_iter()
                .map(|n| {
                    let (path, line) = match n.position {
                        Some(p) => {
                            let path = p.new_path.or(p.old_path);
                            let line = p.new_line.or(p.old_line);
                            (path, line)
                        }
                        None => (None, None),
                    };
                    ForgeNote {
                        id: n.id.to_string(),
                        author: n.author.map(|a| a.username).unwrap_or_default(),
                        body: n.body,
                        path,
                        line,
                    }
                })
                .collect(),
        })
    }
}

#[derive(Debug, Deserialize)]
struct GlNote {
    id: u64,
    #[serde(default)]
    body: String,
    #[serde(default)]
    author: Option<GlAuthor>,
    #[serde(default)]
    system: bool,
    #[serde(default)]
    resolvable: bool,
    #[serde(default)]
    resolved: bool,
    #[serde(default)]
    position: Option<GlPosition>,
}

#[derive(Debug, Deserialize)]
struct GlPosition {
    #[serde(default)]
    new_path: Option<String>,
    #[serde(default)]
    old_path: Option<String>,
    #[serde(default)]
    new_line: Option<u32>,
    #[serde(default)]
    old_line: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct GlDiscussionCreated {
    id: String,
}

#[derive(Debug, Deserialize)]
struct GlNoteCreated {
    id: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::{HeaderMap, HeaderValue};

    #[test]
    fn encodes_project_ids() {
        assert_eq!(
            encode_project_id("entries/entries_crm"),
            "entries%2Fentries_crm"
        );
        assert_eq!(encode_project_id("42"), "42");
        assert_eq!(encode_project_id("/a/b c/"), "a%2Fb%20c");
        assert_eq!(encode_project_id("grp.sub/my-repo"), "grp.sub%2Fmy-repo");
    }

    #[test]
    fn normalizes_base_urls() {
        assert_eq!(normalize_base("https://gitlab.com/"), "https://gitlab.com");
        assert_eq!(normalize_base("https://gl.x/api/v4/"), "https://gl.x");
        assert_eq!(normalize_base(" https://gl.x/sub "), "https://gl.x/sub");
    }

    #[test]
    fn next_page_headers() {
        let mut h = HeaderMap::new();
        assert_eq!(next_page(&h, 1), None);
        h.insert("x-total-pages", HeaderValue::from_static("3"));
        assert_eq!(next_page(&h, 1), Some(2));
        assert_eq!(next_page(&h, 3), None);
        h.insert("x-next-page", HeaderValue::from_static(""));
        assert_eq!(next_page(&h, 1), None);
        h.insert("x-next-page", HeaderValue::from_static("5"));
        assert_eq!(next_page(&h, 4), Some(5));
    }

    #[test]
    fn error_messages() {
        assert_eq!(
            error_message(r#"{"message":"401 Unauthorized"}"#),
            "401 Unauthorized"
        );
        assert_eq!(
            error_message(r#"{"message":{"line_code":["can't be blank"]}}"#),
            r#"{"line_code":["can't be blank"]}"#
        );
        assert_eq!(
            error_message(r#"{"error":"invalid_token"}"#),
            "invalid_token"
        );
        assert_eq!(error_message(""), "empty response body");
        assert_eq!(
            error_message("<html>bad gateway</html>"),
            "<html>bad gateway</html>"
        );
    }

    #[test]
    fn mr_mapping_uses_diff_refs_and_wip() {
        let raw: GlMergeRequest = serde_json::from_value(serde_json::json!({
            "iid": 7, "title": "t", "description": null, "author": {"username": "dev"},
            "source_branch": "feat", "target_branch": "main", "web_url": "u",
            "work_in_progress": true, "labels": ["a", {"name": "b"}], "sha": "listsha",
            "diff_refs": {"base_sha": "b", "start_sha": "s", "head_sha": "h"},
            "state": "opened", "updated_at": "2026-01-01T00:00:00Z"
        }))
        .unwrap();
        let mr = raw.into_model(9);
        assert_eq!(mr.project_id, 9);
        assert!(mr.draft);
        assert_eq!(mr.labels, vec!["a", "b"]);
        assert_eq!(
            (
                mr.base_sha.as_str(),
                mr.start_sha.as_str(),
                mr.head_sha.as_str()
            ),
            ("b", "s", "h")
        );
        assert_eq!(mr.description, "");
    }

    #[test]
    fn mr_mapping_falls_back_to_sha_without_diff_refs() {
        let raw: GlMergeRequest = serde_json::from_value(serde_json::json!({
            "iid": 1, "sha": "abc", "draft": false, "state": "opened"
        }))
        .unwrap();
        let mr = raw.into_model(1);
        assert_eq!(mr.head_sha, "abc");
        assert_eq!(mr.base_sha, "");
        assert!(!mr.draft);
    }

    #[test]
    fn line_note_form_includes_only_present_lines() {
        let pos = DiffPosition {
            base_sha: "b".into(),
            start_sha: "s".into(),
            head_sha: "h".into(),
            old_path: "a.py".into(),
            new_path: "a.py".into(),
            old_line: None,
            new_line: Some(12),
        };
        let f = line_note_form("hi", &pos);
        assert!(f.contains(&("position[position_type]", "text".into())));
        assert!(f.contains(&("position[new_line]", "12".into())));
        assert!(!f.iter().any(|(k, _)| *k == "position[old_line]"));
    }
}
