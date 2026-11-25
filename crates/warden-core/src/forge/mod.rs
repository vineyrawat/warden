// OWNER: forge
//! Forge abstraction. Only `poster.rs` (Warden's own code path) calls the write methods; Claude never
//! sees a forge token or a forge-writing tool (hard rule 1). Tests use wiremock only (hard rule 3).
//!
//! Error convention for callers (poster): every non-2xx forge response becomes
//! [`WardenError::Forge`] with the real HTTP status. A rejected line position is reported as
//! `status: 400` by both providers (GitHub's 422 "pull_request_review_thread.line" error is mapped to
//! 400 by `GithubProvider::post_line_note`), so `matches!(e, WardenError::Forge { status: 400, .. })`
//! or [`is_position_error`] is enough to trigger the file-level fallback note.

pub mod github;
pub mod gitlab;

use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::{Result, WardenError};
use crate::types::{
    DiffPosition, ForgeKind, MergeRequest, MrReviewStatus, Project, RemoteProjectInfo,
};

pub use github::GithubProvider;
pub use gitlab::GitlabProvider;

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ForgeDiscussion {
    pub id: String,
    pub resolved: bool,
    pub notes: Vec<ForgeNote>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ForgeNote {
    pub id: String,
    pub author: String,
    pub body: String,
    pub path: Option<String>,
    pub line: Option<u32>,
}

#[async_trait]
pub trait ForgeProvider: Send + Sync {
    /// Username of the token owner.
    async fn whoami(&self) -> Result<String>;
    /// (remote_id, name) pairs matching `search`.
    async fn list_projects(&self, search: &str) -> Result<Vec<(String, String)>>;
    /// Projects the token owner can access, most recently active first, with the details the
    /// add-project screen shows (default branch, clone URL). Default: built from `list_projects`.
    async fn list_project_details(&self, search: &str) -> Result<Vec<RemoteProjectInfo>> {
        Ok(self
            .list_projects(search)
            .await?
            .into_iter()
            .map(|(remote_id, name)| RemoteProjectInfo {
                remote_id,
                name,
                ..Default::default()
            })
            .collect())
    }
    /// Names (or wildcard patterns) of protected branches. Default: none.
    async fn protected_branches(&self, _remote_id: &str) -> Result<Vec<String>> {
        Ok(Vec::new())
    }
    /// `Authorization` header value git can use over HTTPS for this forge (Warden-managed clones
    /// only; scoped to the forge host). Default: none.
    fn git_auth_header(&self) -> Option<String> {
        None
    }
    /// Open MRs; fills base/start/head shas from diff_refs.
    async fn list_open_mrs(&self, project: &Project) -> Result<Vec<MergeRequest>>;
    async fn get_mr(&self, project: &Project, iid: u64) -> Result<MergeRequest>;
    async fn list_discussions(&self, project: &Project, iid: u64) -> Result<Vec<ForgeDiscussion>>;
    /// Returns the note/discussion id.
    async fn post_line_note(
        &self,
        project: &Project,
        iid: u64,
        body: &str,
        pos: &DiffPosition,
    ) -> Result<String>;
    async fn post_note(&self, project: &Project, iid: u64, body: &str) -> Result<String>;
    /// File-level fallback for a finding whose line position was rejected. Returns the id and
    /// whether that id is a discussion [`ForgeProvider::resolve_discussion`] can resolve.
    /// Default: a plain note (not resolvable). GitLab overrides it with a position-less discussion.
    async fn post_file_note(
        &self,
        project: &Project,
        iid: u64,
        body: &str,
    ) -> Result<(String, bool)> {
        Ok((self.post_note(project, iid, body).await?, false))
    }
    async fn resolve_discussion(
        &self,
        project: &Project,
        iid: u64,
        discussion_id: &str,
    ) -> Result<()>;
    async fn add_label(&self, project: &Project, iid: u64, label: &str) -> Result<()>;
    /// Removes a label; a label that is not on the MR is not an error. Default: no-op.
    async fn remove_label(&self, _project: &Project, _iid: u64, _label: &str) -> Result<()> {
        Ok(())
    }
    /// Sets the forge's own review state (GitHub review event / GitLab approve-unapprove).
    async fn set_review_status(
        &self,
        _project: &Project,
        _iid: u64,
        _status: MrReviewStatus,
        _body: &str,
    ) -> Result<()> {
        Err(WardenError::Config(
            "this forge does not support setting the review status".into(),
        ))
    }
}

/// Builds the provider for a project's forge. The token is held only by the provider (never logged,
/// never handed to Claude).
pub fn provider_for(project: &Project, token: String) -> Arc<dyn ForgeProvider> {
    match project.forge {
        ForgeKind::Gitlab => Arc::new(GitlabProvider::new(project.base_url.clone(), token)),
        ForgeKind::Github => Arc::new(GithubProvider::new(project.base_url.clone(), token)),
    }
}

/// Same as [`provider_for`] but keyed by forge + base URL (used by `set_forge_token` /
/// `search_remote_projects`, which have no `Project` yet).
pub fn provider_for_host(
    forge: ForgeKind,
    base_url: &str,
    token: String,
) -> Arc<dyn ForgeProvider> {
    match forge {
        ForgeKind::Gitlab => Arc::new(GitlabProvider::new(base_url, token)),
        ForgeKind::Github => Arc::new(GithubProvider::new(base_url, token)),
    }
}

/// True when the forge rejected a line-anchored note because of its position (GitLab 400,
/// GitHub 422 — both surface as `Forge { status: 400 }`; 422 is accepted too for safety).
pub fn is_position_error(err: &WardenError) -> bool {
    matches!(
        err,
        WardenError::Forge {
            status: 400 | 422,
            ..
        }
    )
}

/// True for 401/403 (bad, expired or under-scoped token).
pub fn is_auth_error(err: &WardenError) -> bool {
    matches!(
        err,
        WardenError::Forge {
            status: 401 | 403,
            ..
        }
    )
}

/// Label that opts an MR out of AI review.
pub const NO_AI_REVIEW_LABEL: &str = "no-ai-review";

/// Reason the pipeline should skip reviewing this MR (recorded as `RunStatus::Skipped`), if any.
pub fn skip_reason(mr: &MergeRequest) -> Option<&'static str> {
    if mr.draft {
        Some("draft merge request")
    } else if mr
        .labels
        .iter()
        .any(|l| l.eq_ignore_ascii_case(NO_AI_REVIEW_LABEL))
    {
        Some("labelled no-ai-review")
    } else {
        None
    }
}

/// Cuts a forge error body down to something safe to show/log (no multi-KB HTML pages).
pub(crate) fn truncate_msg(s: &str) -> String {
    const MAX: usize = 500;
    let s = s.trim();
    if s.len() <= MAX {
        return s.to_string();
    }
    let mut end = MAX;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::PostingMode;

    fn project(forge: ForgeKind) -> Project {
        Project {
            id: 1,
            forge,
            base_url: "https://example.invalid".into(),
            remote_id: "group/repo".into(),
            name: "repo".into(),
            local_path: "/tmp/repo".into(),
            target_branches: vec![],
            mode: PostingMode::Draft,
            poll_interval_secs: 60,
            enabled: true,
        }
    }

    #[test]
    fn provider_for_builds_without_runtime() {
        // Must not panic outside a tokio runtime (octocrab is built lazily per request).
        let _gl = provider_for(&project(ForgeKind::Gitlab), "t".into());
        let _gh = provider_for(&project(ForgeKind::Github), "t".into());
        let _h = provider_for_host(ForgeKind::Github, "https://github.com", "t".into());
    }

    #[test]
    fn position_and_auth_errors() {
        let e400 = WardenError::Forge {
            status: 400,
            msg: "line_code".into(),
        };
        let e422 = WardenError::Forge {
            status: 422,
            msg: "x".into(),
        };
        let e401 = WardenError::Forge {
            status: 401,
            msg: "x".into(),
        };
        assert!(is_position_error(&e400));
        assert!(is_position_error(&e422));
        assert!(!is_position_error(&e401));
        assert!(is_auth_error(&e401));
        assert!(!is_auth_error(&WardenError::Http("boom".into())));
    }

    #[test]
    fn skip_reasons() {
        let mut mr = MergeRequest::default();
        assert_eq!(skip_reason(&mr), None);
        mr.labels = vec!["No-AI-Review".into()];
        assert_eq!(skip_reason(&mr), Some("labelled no-ai-review"));
        mr.draft = true;
        assert_eq!(skip_reason(&mr), Some("draft merge request"));
    }

    #[test]
    fn truncates_long_messages_on_char_boundary() {
        let long = "é".repeat(400);
        let t = truncate_msg(&long);
        assert!(t.len() <= 504);
        assert!(t.ends_with('…'));
        assert_eq!(truncate_msg("  short "), "short");
    }
}
