// OWNER: forge
//! Detects new MRs / new head SHAs / closed MRs for a project. Draft MRs and MRs labelled
//! "no-ai-review" are still reported (so the pipeline can record `RunStatus::Skipped`); use
//! [`MrChange::skip_reason`] to tell them apart.
//!
//! Change rules for each open MR (after the `target_branches` filter), compared to stored state:
//! - already reviewed at the current head (`last_reviewed_sha == head_sha`) -> nothing;
//! - not stored yet -> `New`;
//! - head moved -> `NewCommits { since: last_reviewed_sha }`, or `New` if it was never reviewed;
//! - head unchanged but it just became reviewable (was draft / no-ai-review / closed before, now is
//!   not) -> `New` (or `NewCommits` when an older head was reviewed);
//! - otherwise nothing (it was reported on an earlier poll).
//!
//! Stored MRs that are `opened` but no longer in the open list -> `Closed` (the MR's real state is
//! fetched with `get_mr` when possible so merged vs closed is recorded correctly).

use std::collections::HashMap;

use async_trait::async_trait;
use globset::{Glob, GlobSet, GlobSetBuilder};

use crate::db::Db;
use crate::error::Result;
use crate::forge::{skip_reason, ForgeProvider};
use crate::types::{MergeRequest, Project};

#[derive(Debug, Clone, PartialEq)]
pub enum MrChangeReason {
    New,
    NewCommits { since: String },
    Closed,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MrChange {
    pub mr: MergeRequest,
    pub reason: MrChangeReason,
}

impl MrChange {
    /// `Some(reason)` when the pipeline should not review this MR (draft or `no-ai-review` label);
    /// always `None` for `Closed`.
    pub fn skip_reason(&self) -> Option<&'static str> {
        match self.reason {
            MrChangeReason::Closed => None,
            _ => skip_reason(&self.mr),
        }
    }

    /// True for `New` / `NewCommits` changes that are not skipped.
    pub fn needs_review(&self) -> bool {
        !matches!(self.reason, MrChangeReason::Closed) && self.skip_reason().is_none()
    }
}

/// Stored-state seam so the poller does not depend on a particular storage implementation.
/// `Db` implements it; tests use an in-memory fake.
#[async_trait]
pub trait MrStore: Send + Sync {
    /// All MRs known for the project (any state), including `last_reviewed_sha`.
    async fn stored_mrs(&self, project_id: i64) -> Result<Vec<MergeRequest>>;
    /// Insert or update by (project_id, iid); returns the row id.
    async fn save_mr(&self, mr: &MergeRequest) -> Result<i64>;
}

#[async_trait]
impl MrStore for Db {
    async fn stored_mrs(&self, project_id: i64) -> Result<Vec<MergeRequest>> {
        self.list_mrs(Some(project_id)).await
    }

    async fn save_mr(&self, mr: &MergeRequest) -> Result<i64> {
        self.upsert_mr(mr).await
    }
}

#[derive(Debug, Clone, Default)]
pub struct Poller;

impl Poller {
    pub fn new() -> Self {
        Poller
    }

    /// Lists open MRs (filtered by `project.target_branches` globs), upserts them, and returns what changed.
    pub async fn poll_project(
        &self,
        db: &Db,
        provider: &dyn ForgeProvider,
        project: &Project,
    ) -> Result<Vec<MrChange>> {
        self.poll_with_store(db, provider, project).await
    }

    /// Same as [`Poller::poll_project`] over any [`MrStore`]. Every open MR is saved (fresh
    /// metadata, preserved `id` / `last_reviewed_sha`); closed ones are saved with their final state.
    /// Returned MRs carry the stored row id.
    pub async fn poll_with_store(
        &self,
        store: &dyn MrStore,
        provider: &dyn ForgeProvider,
        project: &Project,
    ) -> Result<Vec<MrChange>> {
        let filter = BranchFilter::new(&project.target_branches)?;
        let open: Vec<MergeRequest> = provider
            .list_open_mrs(project)
            .await?
            .into_iter()
            .filter(|mr| filter.matches(&mr.target_branch))
            .collect();
        let stored = store.stored_mrs(project.id).await?;

        let stored_by_iid: HashMap<u64, &MergeRequest> =
            stored.iter().map(|m| (m.iid, m)).collect();
        let mut merged_open = Vec::with_capacity(open.len());
        for mut mr in open {
            mr.project_id = project.id;
            if let Some(s) = stored_by_iid.get(&mr.iid) {
                mr.id = s.id;
                if mr.last_reviewed_sha.is_none() {
                    mr.last_reviewed_sha = s.last_reviewed_sha.clone();
                }
            }
            merged_open.push(mr);
        }

        let mut changes = detect_changes(&merged_open, &stored);

        // Persist every open MR so metadata (title, labels, shas) stays fresh.
        let mut ids: HashMap<u64, i64> = HashMap::new();
        for mr in &merged_open {
            let id = store.save_mr(mr).await?;
            ids.insert(mr.iid, id);
        }

        for change in &mut changes {
            match change.reason {
                MrChangeReason::Closed => {
                    // Learn merged vs closed; keep the stored row if the forge call fails.
                    match provider.get_mr(project, change.mr.iid).await {
                        Ok(remote) if remote.state != "opened" => {
                            let mut m = remote;
                            m.id = change.mr.id;
                            m.project_id = project.id;
                            m.last_reviewed_sha = change.mr.last_reviewed_sha.clone();
                            change.mr = m;
                        }
                        Ok(_) => {} // still open but filtered out (target branch changed)
                        Err(e) => {
                            tracing::debug!(iid = change.mr.iid, error = %e, "get_mr for closed MR failed")
                        }
                    }
                    if change.mr.state == "opened" {
                        change.mr.state = "closed".into();
                    }
                    change.mr.id = store.save_mr(&change.mr).await?;
                }
                _ => {
                    if let Some(id) = ids.get(&change.mr.iid) {
                        change.mr.id = *id;
                    }
                }
            }
        }
        Ok(changes)
    }

    /// Pure helper for callers that only have `iid -> (head_sha, last_reviewed_sha)` pairs.
    pub fn changes_from_pairs(
        open: &[MergeRequest],
        stored: &HashMap<u64, (String, Option<String>)>,
    ) -> Vec<MrChange> {
        let stored: Vec<MergeRequest> = stored
            .iter()
            .map(|(iid, (head, reviewed))| MergeRequest {
                iid: *iid,
                head_sha: head.clone(),
                last_reviewed_sha: reviewed.clone(),
                state: "opened".into(),
                ..Default::default()
            })
            .collect();
        detect_changes(open, &stored)
    }
}

/// Compares the forge's open MRs with stored MRs (see the module docs for the rules).
/// `open` should already be filtered by target branch. Order: open-MR changes by iid, then closed.
pub fn detect_changes(open: &[MergeRequest], stored: &[MergeRequest]) -> Vec<MrChange> {
    let stored_by_iid: HashMap<u64, &MergeRequest> = stored.iter().map(|m| (m.iid, m)).collect();
    let mut changes = Vec::new();

    let mut open_sorted: Vec<&MergeRequest> = open.iter().collect();
    open_sorted.sort_by_key(|m| m.iid);
    for mr in open_sorted {
        let prev = stored_by_iid.get(&mr.iid).copied();
        let reviewed = mr
            .last_reviewed_sha
            .as_deref()
            .or_else(|| prev.and_then(|p| p.last_reviewed_sha.as_deref()))
            .filter(|s| !s.is_empty());

        if reviewed == Some(mr.head_sha.as_str()) {
            continue;
        }
        let incremental = |r: Option<&str>| match r {
            Some(since) => MrChangeReason::NewCommits {
                since: since.to_string(),
            },
            None => MrChangeReason::New,
        };
        let reason = match prev {
            None => Some(MrChangeReason::New),
            Some(p) if p.head_sha != mr.head_sha => Some(incremental(reviewed)),
            Some(p) => {
                let was_blocked = p.state != "opened" || skip_reason(p).is_some();
                let now_reviewable = skip_reason(mr).is_none();
                (was_blocked && now_reviewable).then(|| incremental(reviewed))
            }
        };
        if let Some(reason) = reason {
            let mut out = mr.clone();
            if out.last_reviewed_sha.is_none() {
                out.last_reviewed_sha = reviewed.map(str::to_string);
            }
            if out.id == 0 {
                if let Some(p) = prev {
                    out.id = p.id;
                }
            }
            changes.push(MrChange { mr: out, reason });
        }
    }

    let open_iids: std::collections::HashSet<u64> = open.iter().map(|m| m.iid).collect();
    let mut closed: Vec<&MergeRequest> = stored
        .iter()
        .filter(|m| m.state == "opened" && !open_iids.contains(&m.iid))
        .collect();
    closed.sort_by_key(|m| m.iid);
    for m in closed {
        changes.push(MrChange {
            mr: m.clone(),
            reason: MrChangeReason::Closed,
        });
    }
    changes
}

/// `target_branches` glob filter; an empty list watches every branch.
pub struct BranchFilter {
    set: Option<GlobSet>,
}

impl BranchFilter {
    pub fn new(globs: &[String]) -> Result<Self> {
        let globs: Vec<&String> = globs.iter().filter(|g| !g.trim().is_empty()).collect();
        if globs.is_empty() {
            return Ok(BranchFilter { set: None });
        }
        let mut b = GlobSetBuilder::new();
        for g in globs {
            b.add(Glob::new(g.trim())?);
        }
        Ok(BranchFilter {
            set: Some(b.build()?),
        })
    }

    pub fn matches(&self, branch: &str) -> bool {
        self.set.as_ref().is_none_or(|s| s.is_match(branch))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mr(iid: u64, head: &str) -> MergeRequest {
        MergeRequest {
            iid,
            head_sha: head.into(),
            target_branch: "main".into(),
            state: "opened".into(),
            ..Default::default()
        }
    }

    #[test]
    fn branch_filter() {
        let f = BranchFilter::new(&["main".into(), "release/*".into()]).unwrap();
        assert!(f.matches("main"));
        assert!(f.matches("release/1.2"));
        assert!(!f.matches("develop"));
        assert!(BranchFilter::new(&[]).unwrap().matches("anything"));
        assert!(BranchFilter::new(&["  ".into()])
            .unwrap()
            .matches("anything"));
        assert!(BranchFilter::new(&["[".into()]).is_err());
    }

    #[test]
    fn pairs_helper() {
        let mut stored = HashMap::new();
        stored.insert(1, ("a".to_string(), Some("a".to_string())));
        stored.insert(2, ("b".to_string(), Some("b".to_string())));
        stored.insert(3, ("c".to_string(), None));
        let open = vec![mr(1, "a"), mr(2, "b2"), mr(4, "d")];
        let ch = Poller::changes_from_pairs(&open, &stored);
        assert_eq!(ch.len(), 3);
        assert_eq!(ch[0].mr.iid, 2);
        assert_eq!(
            ch[0].reason,
            MrChangeReason::NewCommits { since: "b".into() }
        );
        assert_eq!(ch[1].mr.iid, 4);
        assert_eq!(ch[1].reason, MrChangeReason::New);
        assert_eq!(ch[2].mr.iid, 3);
        assert_eq!(ch[2].reason, MrChangeReason::Closed);
    }

    #[test]
    fn skip_flags() {
        let mut m = mr(1, "a");
        m.draft = true;
        let c = MrChange {
            mr: m.clone(),
            reason: MrChangeReason::New,
        };
        assert_eq!(c.skip_reason(), Some("draft merge request"));
        assert!(!c.needs_review());
        let c = MrChange {
            mr: m,
            reason: MrChangeReason::Closed,
        };
        assert_eq!(c.skip_reason(), None);
        assert!(!c.needs_review());
        let c = MrChange {
            mr: mr(2, "b"),
            reason: MrChangeReason::New,
        };
        assert!(c.needs_review());
    }
}
