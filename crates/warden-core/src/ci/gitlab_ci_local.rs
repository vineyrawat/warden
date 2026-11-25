// OWNER: ci
//! `gitlab-ci-local` adapter (recipe `use: gitlab-ci-local`, `jobs: [...]`). Not installed on the dev
//! machine: must report CiStatus::NotInstalled, never crash.
//!
//! Detection order: `gitlab-ci-local` on the child PATH, else `npx --no-install gitlab-ci-local
//! --version` (finds a project-local `node_modules/.bin` copy; `--no-install` never downloads).
//! Each listed job runs as `gitlab-ci-local --job <name>`; all jobs run even if an earlier one fails
//! and the recipe fails if any job failed.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use crate::config::RecipeConfig;

/// Adapter name used in `.warden.yml` (`use: gitlab-ci-local`).
pub const ADAPTER: &str = "gitlab-ci-local";
/// Shown in the job's failure excerpt / log when the tool is missing.
pub const INSTALL_HINT: &str =
    "gitlab-ci-local is not installed. Install it with `npm i -g gitlab-ci-local` (it also needs docker for most jobs).";

const NPX_PROBE_TIMEOUT: Duration = Duration::from_secs(20);

/// True if `recipe` uses this adapter.
pub fn is_adapter(recipe: &RecipeConfig) -> bool {
    recipe
        .use_
        .as_deref()
        .map(|u| u.trim().eq_ignore_ascii_case(ADAPTER))
        .unwrap_or(false)
}

/// True if `gitlab-ci-local` is on the current process PATH (cheap, sync; no npx probe).
pub fn is_installed() -> bool {
    which::which(ADAPTER).is_ok()
}

/// Finds `gitlab-ci-local` on the given PATH value.
pub fn find_on_path(path_env: Option<&OsStr>, cwd: &Path) -> Option<PathBuf> {
    which::which_in(ADAPTER, path_env, cwd).ok()
}

/// How to invoke the tool: `["/path/to/gitlab-ci-local"]`, `["npx", "--no-install",
/// "gitlab-ci-local"]`, or None when it is not available. `path_env` is the PATH the job will run
/// with; `cwd` is where the job runs (npx looks for a local install there).
pub async fn detect(path_env: Option<&OsStr>, cwd: &Path) -> Option<Vec<String>> {
    if let Some(p) = find_on_path(path_env, cwd) {
        return Some(vec![p.to_string_lossy().into_owned()]);
    }
    let npx = which::which_in("npx", path_env, cwd).ok()?;
    let mut cmd = tokio::process::Command::new(&npx);
    cmd.args(["--no-install", ADAPTER, "--version"])
        .current_dir(cwd)
        .env("npm_config_yes", "false")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    if let Some(p) = path_env {
        cmd.env("PATH", p);
    }
    match tokio::time::timeout(NPX_PROBE_TIMEOUT, cmd.status()).await {
        Ok(Ok(st)) if st.success() => Some(vec![
            npx.to_string_lossy().into_owned(),
            "--no-install".into(),
            ADAPTER.into(),
        ]),
        _ => None,
    }
}

/// Shell command running each of the recipe's jobs with `program` (the output of [`detect`]), or
/// None if the recipe is not a gitlab-ci-local recipe or lists no jobs.
pub fn command_for(recipe: &RecipeConfig, program: &[String]) -> Option<String> {
    if !is_adapter(recipe) || recipe.jobs.is_empty() || program.is_empty() {
        return None;
    }
    let prog = program
        .iter()
        .map(|p| quote(p))
        .collect::<Vec<_>>()
        .join(" ");
    let jobs = recipe
        .jobs
        .iter()
        .filter(|j| !j.trim().is_empty())
        .collect::<Vec<_>>();
    if jobs.is_empty() {
        return None;
    }
    if jobs.len() == 1 {
        return Some(format!("{prog} --job {}", quote(jobs[0])));
    }
    let mut s = String::from("rc=0; ");
    for j in jobs {
        s.push_str(&format!("{prog} --job {} || rc=1; ", quote(j)));
    }
    s.push_str("exit $rc");
    Some(s)
}

/// Display form of the command (as configured, before detection), used for NotInstalled jobs.
pub fn display_command(recipe: &RecipeConfig) -> String {
    let jobs = recipe
        .jobs
        .iter()
        .map(|j| format!("--job {}", quote(j)))
        .collect::<Vec<_>>()
        .join(" ");
    format!("{ADAPTER} {jobs}").trim().to_string()
}

fn quote(s: &str) -> String {
    shlex::try_quote(s)
        .map(|c| c.into_owned())
        .unwrap_or_else(|_| format!("'{}'", s.replace(['\0', '\''], "")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recipe(jobs: &[&str]) -> RecipeConfig {
        RecipeConfig {
            name: "gitlab-ci".into(),
            run: None,
            use_: Some("gitlab-ci-local".into()),
            jobs: jobs.iter().map(|s| s.to_string()).collect(),
            cwd: None,
            timeout: None,
            env: Default::default(),
        }
    }

    #[test]
    fn single_and_multi_job_commands() {
        let prog = vec!["gitlab-ci-local".to_string()];
        assert_eq!(
            command_for(&recipe(&["build"]), &prog).unwrap(),
            "gitlab-ci-local --job build"
        );
        assert_eq!(
            command_for(&recipe(&["build", "unit tests"]), &prog).unwrap(),
            "rc=0; gitlab-ci-local --job build || rc=1; gitlab-ci-local --job 'unit tests' || rc=1; exit $rc"
        );
        assert!(command_for(&recipe(&[]), &prog).is_none());
        let mut not_adapter = recipe(&["build"]);
        not_adapter.use_ = None;
        assert!(command_for(&not_adapter, &prog).is_none());
        let npx: Vec<String> = ["npx", "--no-install", "gitlab-ci-local"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(
            command_for(&recipe(&["lint"]), &npx).unwrap(),
            "npx --no-install gitlab-ci-local --job lint"
        );
    }

    #[test]
    fn display() {
        assert_eq!(
            display_command(&recipe(&["build", "test"])),
            "gitlab-ci-local --job build --job test"
        );
    }

    #[tokio::test]
    async fn detect_with_empty_path_is_none() {
        let dir = tempfile::tempdir().unwrap();
        let empty = dir.path().join("empty-bin");
        std::fs::create_dir_all(&empty).unwrap();
        assert!(detect(Some(empty.as_os_str()), dir.path()).await.is_none());
    }
}
