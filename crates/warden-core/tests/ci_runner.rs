//! CiRunner integration tests: real subprocesses (`/bin/sh -c` for speed and determinism; one test
//! uses the default `/bin/zsh -lc`), temp worktrees, fake `bench` / `gitlab-ci-local` executables.

use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::broadcast;
use warden_core::ci::{gitlab_ci_local, CiRunner, RecipeContext, SKIP_NO_FILES};
use warden_core::config::{CiConfig, RecipeConfig};
use warden_core::{AppEvent, CiJob, CiStatus, EventBus};

const RUN: i64 = 42;

struct Env {
    _tmp: tempfile::TempDir,
    base: PathBuf,
    worktree: PathBuf,
    logs: PathBuf,
    bus: EventBus,
}

fn env() -> Env {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().canonicalize().unwrap();
    let worktree = base.join("wt");
    std::fs::create_dir_all(worktree.join("sub")).unwrap();
    Env {
        _tmp: tmp,
        logs: base.join("logs"),
        worktree,
        base,
        bus: EventBus::new(),
    }
}

fn sh_runner(e: &Env) -> CiRunner {
    CiRunner::new(e.bus.clone(), &e.logs).with_shell(vec!["/bin/sh".into(), "-c".into()])
}

fn recipe(name: &str, run: &str) -> RecipeConfig {
    RecipeConfig {
        name: name.into(),
        run: Some(run.into()),
        use_: None,
        jobs: vec![],
        cwd: None,
        timeout: None,
        env: BTreeMap::new(),
    }
}

fn ctx(files: &[&str]) -> RecipeContext {
    RecipeContext {
        changed_files: files.iter().map(|s| s.to_string()).collect(),
        base_sha: "base000".into(),
        head_sha: "head111".into(),
    }
}

fn drain(rx: &mut broadcast::Receiver<AppEvent>) -> Vec<AppEvent> {
    let mut out = vec![];
    while let Ok(ev) = rx.try_recv() {
        out.push(ev);
    }
    out
}

fn log_lines(events: &[AppEvent], job_id: i64) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match e {
            AppEvent::CiLog {
                job_id: j, line, ..
            } if *j == job_id => Some(line.clone()),
            _ => None,
        })
        .collect()
}

fn job_updates(events: &[AppEvent]) -> Vec<CiJob> {
    events
        .iter()
        .filter_map(|e| match e {
            AppEvent::CiJobUpdated { job } => Some(job.clone()),
            _ => None,
        })
        .collect()
}

fn write_exe(path: &Path, body: &str) {
    std::fs::write(path, body).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[tokio::test]
async fn success_streams_stdout_and_stderr_to_events_and_log() {
    let e = env();
    let mut rx = e.bus.subscribe();
    let job = sh_runner(&e)
        .run_one(
            RUN,
            &e.worktree,
            &recipe("hello", "echo out-line; echo err-line >&2"),
            &ctx(&[]),
        )
        .await
        .unwrap();

    assert_eq!(job.status, CiStatus::Passed);
    assert_eq!(job.run_id, RUN);
    assert_eq!(job.id, 0, "no allocator => id 0");
    assert_eq!(job.kind, "recipe");
    assert_eq!(job.command, "echo out-line; echo err-line >&2");
    assert!(job.failure_excerpt.is_none());
    assert_eq!(
        PathBuf::from(&job.log_path),
        e.logs.join("42").join("hello.log")
    );

    let log = std::fs::read_to_string(&job.log_path).unwrap();
    assert!(log.starts_with("$ echo out-line"), "{log}");
    assert!(
        log.contains("out-line\n") && log.contains("err-line\n"),
        "{log}"
    );
    assert!(log.contains("[warden] exit status 0"), "{log}");

    let events = drain(&mut rx);
    let lines = log_lines(&events, 0);
    assert!(lines.contains(&"out-line".to_string()), "{lines:?}");
    assert!(lines.contains(&"err-line".to_string()), "{lines:?}");
    let updates = job_updates(&events);
    assert_eq!(updates.first().unwrap().status, CiStatus::Running);
    assert_eq!(updates.last().unwrap().status, CiStatus::Passed);
}

#[tokio::test]
async fn failure_sets_status_and_excerpt_around_first_error() {
    let e = env();
    let mut cmd = String::new();
    for i in 0..60 {
        cmd.push_str(&format!("echo setup {i}; "));
    }
    cmd.push_str("echo 'E   AssertionError: expected 2'; ");
    for i in 0..60 {
        cmd.push_str(&format!("echo after {i}; "));
    }
    cmd.push_str("exit 3");
    let job = sh_runner(&e)
        .run_one(RUN, &e.worktree, &recipe("tests", &cmd), &ctx(&[]))
        .await
        .unwrap();
    assert_eq!(job.status, CiStatus::Failed);
    let ex = job.failure_excerpt.unwrap();
    let lines: Vec<&str> = ex.lines().collect();
    assert_eq!(lines.len(), 40, "{ex}");
    assert_eq!(lines[0], "setup 50");
    assert_eq!(lines[10], "E   AssertionError: expected 2");
    assert_eq!(lines[39], "after 28");
    let log = std::fs::read_to_string(&job.log_path).unwrap();
    assert!(log.contains("[warden] exit status 3"), "{log}");
}

#[tokio::test]
async fn failure_without_error_words_uses_last_40_lines() {
    let e = env();
    let job = sh_runner(&e)
        .run_one(
            RUN,
            &e.worktree,
            &recipe(
                "quiet",
                "i=0; while [ $i -lt 100 ]; do echo row $i; i=$((i+1)); done; exit 1",
            ),
            &ctx(&[]),
        )
        .await
        .unwrap();
    assert_eq!(job.status, CiStatus::Failed);
    let ex = job.failure_excerpt.unwrap();
    let lines: Vec<&str> = ex.lines().collect();
    assert_eq!(lines.len(), 40);
    assert_eq!(lines[0], "row 60");
    assert_eq!(lines[39], "row 99");

    // no output at all: the excerpt still says what happened
    let job = sh_runner(&e)
        .run_one(RUN, &e.worktree, &recipe("silent", "exit 7"), &ctx(&[]))
        .await
        .unwrap();
    assert_eq!(job.status, CiStatus::Failed);
    assert!(job.failure_excerpt.unwrap().contains("exit status 7"));
}

#[tokio::test]
async fn timeout_kills_the_whole_process_group() {
    let e = env();
    let pidfile = e.base.join("bg.pid");
    let mut r = recipe(
        "slow",
        &format!(
            "echo started; sleep 30 & echo $! > '{}'; wait",
            pidfile.display()
        ),
    );
    r.timeout = Some("1s".into());
    let started = Instant::now();
    let job = sh_runner(&e)
        .run_one(RUN, &e.worktree, &r, &ctx(&[]))
        .await
        .unwrap();
    let took = started.elapsed();

    assert_eq!(job.status, CiStatus::TimedOut);
    assert!(took < Duration::from_secs(8), "took {took:?}");
    assert!(job.duration_ms >= 1000);
    let ex = job.failure_excerpt.unwrap();
    assert!(ex.contains("timed out after 1s"), "{ex}");
    assert!(ex.contains("started"), "{ex}");

    // the background `sleep 30` (same process group) is gone
    let pid: i32 = std::fs::read_to_string(&pidfile)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    let alive = unsafe { libc::kill(pid, 0) } == 0;
    assert!(!alive, "background child {pid} survived the timeout");
}

#[tokio::test]
async fn shell_exit_with_background_child_holding_pipes_still_finishes() {
    let e = env();
    let mut r = recipe("bg", "echo hi; sleep 30 &");
    r.timeout = Some("20s".into());
    let started = Instant::now();
    let job = sh_runner(&e)
        .run_one(RUN, &e.worktree, &r, &ctx(&[]))
        .await
        .unwrap();
    assert_eq!(job.status, CiStatus::Passed);
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "{:?}",
        started.elapsed()
    );
}

#[tokio::test]
async fn placeholders_render_shell_quoted_filtered_lists() {
    let e = env();
    let r = recipe(
        "print",
        "printf '<%s>\\n' {changed_py}; echo {base_sha}..{head_sha}",
    );
    let job = sh_runner(&e)
        .run_one(
            RUN,
            &e.worktree,
            &r,
            &ctx(&["a.py", "dir/my file.py", "x.ts", "it's.py"]),
        )
        .await
        .unwrap();
    assert_eq!(job.status, CiStatus::Passed, "{:?}", job.failure_excerpt);
    assert_eq!(
        job.command,
        "printf '<%s>\\n' a.py 'dir/my file.py' \"it's.py\"; echo base000..head111"
    );
    let log = std::fs::read_to_string(&job.log_path).unwrap();
    assert!(
        log.contains("<a.py>\n<dir/my file.py>\n<it's.py>\n"),
        "{log}"
    );
    assert!(log.contains("base000..head111"), "{log}");
}

#[tokio::test]
async fn empty_file_list_skips_without_running() {
    let e = env();
    let marker = e.base.join("ran");
    let r = recipe(
        "lint",
        &format!("touch '{}'; ruff check {{changed_py}}", marker.display()),
    );
    let mut rx = e.bus.subscribe();
    let job = sh_runner(&e)
        .run_one(RUN, &e.worktree, &r, &ctx(&["README.md", "ui/app.ts"]))
        .await
        .unwrap();
    assert_eq!(job.status, CiStatus::Skipped);
    assert_eq!(job.failure_excerpt.as_deref(), Some(SKIP_NO_FILES));
    assert!(!marker.exists(), "skipped recipes must not run");
    let log = std::fs::read_to_string(&job.log_path).unwrap();
    assert!(log.contains("no matching files"), "{log}");
    let updates = job_updates(&drain(&mut rx));
    assert_eq!(updates.len(), 1);
    assert_eq!(updates[0].status, CiStatus::Skipped);
}

#[tokio::test]
async fn environment_is_clean_plus_recipe_env_and_cwd_is_respected() {
    let e = env();
    let mut r = recipe("env", "pwd; env");
    r.cwd = Some("sub".into());
    r.env
        .insert("WARDEN_RECIPE_VAR".into(), "recipe-value".into());
    let job = sh_runner(&e)
        .with_env("WARDEN_EXTRA_BASE", "base-value")
        .run_one(RUN, &e.worktree, &r, &ctx(&[]))
        .await
        .unwrap();
    assert_eq!(job.status, CiStatus::Passed);
    let log = std::fs::read_to_string(&job.log_path).unwrap();
    assert!(
        log.contains(&format!("{}\n", e.worktree.join("sub").display())),
        "{log}"
    );
    assert!(log.contains("WARDEN_RECIPE_VAR=recipe-value"), "{log}");
    assert!(log.contains("WARDEN_EXTRA_BASE=base-value"), "{log}");
    assert!(log.contains("PATH="), "{log}");
    assert!(log.contains("HOME="), "{log}");
    // cargo sets these for the test process; they must not leak into jobs
    assert!(!log.contains("CARGO_MANIFEST_DIR"), "{log}");
    assert!(!log.contains("CARGO_PKG_NAME"), "{log}");
    let allowed = [
        "PATH",
        "HOME",
        "USER",
        "LANG",
        "SHELL",
        "WARDEN_RECIPE_VAR",
        "WARDEN_EXTRA_BASE",
        "PWD",
        "SHLVL",
        "_",
        "OLDPWD",
    ];
    for line in log.lines().skip(2) {
        if let Some((k, _)) = line.split_once('=') {
            if line.starts_with("[warden]") {
                continue;
            }
            assert!(
                allowed.contains(&k),
                "unexpected env var {k} in job env:\n{log}"
            );
        }
    }
}

#[tokio::test]
async fn bad_cwd_and_missing_command_fail_cleanly() {
    let e = env();
    let mut r = recipe("escape", "echo hi");
    r.cwd = Some("../..".into());
    let job = sh_runner(&e)
        .run_one(RUN, &e.worktree, &r, &ctx(&[]))
        .await
        .unwrap();
    assert_eq!(job.status, CiStatus::Failed);
    assert!(job.failure_excerpt.unwrap().contains("escapes"));

    let mut r = recipe("missing-dir", "echo hi");
    r.cwd = Some("nope".into());
    let job = sh_runner(&e)
        .run_one(RUN, &e.worktree, &r, &ctx(&[]))
        .await
        .unwrap();
    assert_eq!(job.status, CiStatus::Failed);

    let mut r = recipe("empty", "");
    r.run = None;
    let job = sh_runner(&e)
        .run_one(RUN, &e.worktree, &r, &ctx(&[]))
        .await
        .unwrap();
    assert_eq!(job.status, CiStatus::Failed);
    assert!(job.failure_excerpt.unwrap().contains("neither"));

    let mut r = recipe("act", "");
    r.run = None;
    r.use_ = Some("act".into());
    let job = sh_runner(&e)
        .run_one(RUN, &e.worktree, &r, &ctx(&[]))
        .await
        .unwrap();
    assert_eq!(job.status, CiStatus::Skipped);
    assert!(job.failure_excerpt.unwrap().contains("unsupported adapter"));

    // a shell that does not exist
    let job = CiRunner::new(e.bus.clone(), &e.logs)
        .with_shell(vec!["/nonexistent/shell".into(), "-c".into()])
        .run_one(RUN, &e.worktree, &recipe("noshell", "echo hi"), &ctx(&[]))
        .await
        .unwrap();
    assert_eq!(job.status, CiStatus::Failed);
    assert!(job.failure_excerpt.unwrap().contains("failed to start"));
}

#[tokio::test]
async fn invalid_timeout_falls_back_to_default() {
    let e = env();
    let mut r = recipe("t", "echo ok");
    r.timeout = Some("soon".into());
    let job = sh_runner(&e)
        .run_one(RUN, &e.worktree, &r, &ctx(&[]))
        .await
        .unwrap();
    assert_eq!(job.status, CiStatus::Passed);
    let log = std::fs::read_to_string(&job.log_path).unwrap();
    assert!(
        log.contains("invalid timeout \"soon\", using 900s"),
        "{log}"
    );
}

#[tokio::test]
async fn reruns_get_distinct_log_files() {
    let e = env();
    let runner = sh_runner(&e);
    let a = runner
        .run_one(RUN, &e.worktree, &recipe("lint", "echo first"), &ctx(&[]))
        .await
        .unwrap();
    let b = runner
        .run_one(RUN, &e.worktree, &recipe("lint", "echo second"), &ctx(&[]))
        .await
        .unwrap();
    assert!(a.log_path.ends_with("42/lint.log"), "{}", a.log_path);
    assert!(b.log_path.ends_with("42/lint-2.log"), "{}", b.log_path);
    assert!(std::fs::read_to_string(&a.log_path)
        .unwrap()
        .contains("first"));
    assert!(std::fs::read_to_string(&b.log_path)
        .unwrap()
        .contains("second"));
}

#[tokio::test]
async fn lines_stream_before_the_job_finishes() {
    let e = env();
    let mut rx = e.bus.subscribe();
    let runner = sh_runner(&e);
    let wt = e.worktree.clone();
    let started = Instant::now();
    let handle = tokio::spawn(async move {
        runner
            .run_one(
                RUN,
                &wt,
                &recipe("stream", "echo first; sleep 2; echo second"),
                &ctx(&[]),
            )
            .await
    });
    let first_at = loop {
        match tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .unwrap()
            .unwrap()
        {
            AppEvent::CiLog { line, .. } if line == "first" => break started.elapsed(),
            _ => {}
        }
    };
    assert!(
        first_at < Duration::from_millis(1500),
        "first line arrived after {first_at:?}"
    );
    let job = handle.await.unwrap().unwrap();
    assert_eq!(job.status, CiStatus::Passed);
    assert!(job.duration_ms >= 2000);
}

#[tokio::test]
async fn run_all_runs_in_parallel_and_keeps_order() {
    let e = env();
    let cfg = CiConfig {
        recipes: vec![
            recipe("a", "sleep 1; echo a"),
            recipe("b", "sleep 1; echo b; exit 1"),
            recipe("c", "sleep 1; echo c"),
            recipe("d", "ruff {changed_py}"),
        ],
    };
    let started = Instant::now();
    let jobs = sh_runner(&e)
        .run_all(RUN, &e.worktree, &cfg, &ctx(&["x.md"]))
        .await
        .unwrap();
    let took = started.elapsed();
    assert!(took < Duration::from_millis(2500), "not parallel: {took:?}");
    let names: Vec<&str> = jobs.iter().map(|j| j.name.as_str()).collect();
    assert_eq!(names, ["a", "b", "c", "d"]);
    let statuses: Vec<CiStatus> = jobs.iter().map(|j| j.status).collect();
    assert_eq!(
        statuses,
        [
            CiStatus::Passed,
            CiStatus::Failed,
            CiStatus::Passed,
            CiStatus::Skipped
        ]
    );
    // empty config
    let none = sh_runner(&e)
        .run_all(RUN, &e.worktree, &CiConfig::default(), &ctx(&[]))
        .await
        .unwrap();
    assert!(none.is_empty());
}

#[tokio::test]
async fn cancel_kills_running_jobs_of_that_run_only() {
    let e = env();
    let runner = Arc::new(sh_runner(&e));
    let cfg = CiConfig {
        recipes: vec![
            recipe("long1", "sleep 30"),
            recipe("long2", "echo x; sleep 30"),
        ],
    };
    let r2 = runner.clone();
    let wt = e.worktree.clone();
    let started = Instant::now();
    let h = tokio::spawn(async move { r2.run_all(RUN, &wt, &cfg, &ctx(&[])).await });

    // another run that must not be affected
    let r3 = runner.clone();
    let wt = e.worktree.clone();
    let other = tokio::spawn(async move {
        r3.run_one(7, &wt, &recipe("other", "sleep 1; echo done"), &ctx(&[]))
            .await
    });

    tokio::time::sleep(Duration::from_millis(400)).await;
    runner.cancel(RUN);
    runner.cancel(12345); // unknown run: no-op
    let jobs = h.await.unwrap().unwrap();
    assert!(started.elapsed() < Duration::from_secs(8));
    assert!(
        jobs.iter().all(|j| j.status == CiStatus::Cancelled),
        "{jobs:?}"
    );
    assert!(jobs.iter().all(|j| j.failure_excerpt.is_none()));
    let other = other.await.unwrap().unwrap();
    assert_eq!(other.status, CiStatus::Passed);

    // the cancel signal does not stick: a later rerun of the same run works
    let again = runner
        .run_one(RUN, &e.worktree, &recipe("again", "echo ok"), &ctx(&[]))
        .await
        .unwrap();
    assert_eq!(again.status, CiStatus::Passed);
    assert!(
        runner.cancels.lock().unwrap().is_empty(),
        "cancel registry cleaned up"
    );
}

#[tokio::test]
async fn bench_commands_are_serialized_per_bench() {
    let e = env();
    // <base>/bench/{sites, apps/wt}
    let bench = e.base.join("bench");
    std::fs::create_dir_all(bench.join("sites")).unwrap();
    let wt = bench.join("apps").join("wt");
    std::fs::create_dir_all(&wt).unwrap();
    let bin = e.base.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let trace = e.base.join("trace.txt");
    write_exe(
        &bin.join("bench"),
        &format!(
            "#!/bin/sh\necho \"start $1\" >> '{t}'\nsleep 0.6\necho \"end $1\" >> '{t}'\n",
            t = trace.display()
        ),
    );
    let path = format!("{}:/usr/bin:/bin", bin.display());
    let cfg = CiConfig {
        recipes: vec![
            recipe("b1", "bench one"),
            recipe("b2", "bench two"),
            recipe("other", &format!("echo \"other\" >> '{}'", trace.display())),
        ],
    };
    let jobs = sh_runner(&e)
        .with_env("PATH", path)
        .run_all(RUN, &wt, &cfg, &ctx(&[]))
        .await
        .unwrap();
    assert!(
        jobs.iter().all(|j| j.status == CiStatus::Passed),
        "{jobs:?}"
    );
    let t = std::fs::read_to_string(&trace).unwrap();
    let bench_lines: Vec<&str> = t.lines().filter(|l| *l != "other").collect();
    assert_eq!(bench_lines.len(), 4, "{t}");
    // never interleaved: start X is always followed by end X
    assert!(
        bench_lines[0].starts_with("start ") && bench_lines[1].starts_with("end "),
        "{t}"
    );
    assert_eq!(&bench_lines[0][6..], &bench_lines[1][4..], "{t}");
    assert!(
        bench_lines[2].starts_with("start ") && bench_lines[3].starts_with("end "),
        "{t}"
    );
    // the non-bench job did not wait for the lock: it ran before the first bench job ended
    let other_pos = t.lines().position(|l| l == "other").unwrap();
    assert!(other_pos < 3, "{t}");
}

#[tokio::test]
async fn job_id_allocator_ids_flow_into_events_and_jobs() {
    let e = env();
    let next = Arc::new(AtomicI64::new(100));
    let seen = Arc::new(std::sync::Mutex::new(Vec::<CiJob>::new()));
    let (n2, s2) = (next.clone(), seen.clone());
    let runner = sh_runner(&e).with_job_id_allocator(move |job: CiJob| {
        let n = n2.clone();
        let s = s2.clone();
        async move {
            s.lock().unwrap().push(job);
            Ok(n.fetch_add(1, Ordering::SeqCst))
        }
    });
    let mut rx = e.bus.subscribe();
    let cfg = CiConfig {
        recipes: vec![
            recipe("x", "echo from-x"),
            recipe("y", "echo from-y"),
            recipe("z", "lint {changed_ts}"),
        ],
    };
    let jobs = runner
        .run_all(RUN, &e.worktree, &cfg, &ctx(&[]))
        .await
        .unwrap();
    let mut ids: Vec<i64> = jobs.iter().map(|j| j.id).collect();
    ids.sort();
    assert_eq!(ids, [100, 101, 102]);
    let events = drain(&mut rx);
    for j in &jobs {
        let lines = log_lines(&events, j.id);
        match j.name.as_str() {
            "x" => assert!(lines.contains(&"from-x".to_string()), "{lines:?}"),
            "y" => assert!(lines.contains(&"from-y".to_string()), "{lines:?}"),
            _ => assert!(
                lines.iter().any(|l| l.contains("no matching files")),
                "{lines:?}"
            ),
        }
    }
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 3);
    for j in seen.iter() {
        if j.name == "z" {
            assert_eq!(j.status, CiStatus::Skipped);
        } else {
            assert_eq!(j.status, CiStatus::Running);
        }
        assert_eq!(j.run_id, RUN);
    }
}

#[tokio::test]
async fn allocator_error_falls_back_to_zero() {
    let e = env();
    let runner = sh_runner(&e).with_job_id_allocator(|_job: CiJob| async {
        Err(warden_core::WardenError::Other("db down".into()))
    });
    let job = runner
        .run_one(RUN, &e.worktree, &recipe("x", "echo hi"), &ctx(&[]))
        .await
        .unwrap();
    assert_eq!(job.id, 0);
    assert_eq!(job.status, CiStatus::Passed);
}

fn glc_recipe(jobs: &[&str]) -> RecipeConfig {
    RecipeConfig {
        name: "gitlab-ci".into(),
        run: None,
        use_: Some("gitlab-ci-local".into()),
        jobs: jobs.iter().map(|s| s.to_string()).collect(),
        cwd: None,
        timeout: None,
        env: BTreeMap::new(),
    }
}

#[tokio::test]
async fn gitlab_ci_local_not_installed() {
    let e = env();
    let empty_bin = e.base.join("empty-bin");
    std::fs::create_dir_all(&empty_bin).unwrap();
    let mut rx = e.bus.subscribe();
    let job = sh_runner(&e)
        .with_env("PATH", empty_bin.to_string_lossy())
        .run_one(RUN, &e.worktree, &glc_recipe(&["build", "test"]), &ctx(&[]))
        .await
        .unwrap();
    assert_eq!(job.status, CiStatus::NotInstalled);
    assert_eq!(job.kind, "gitlab_ci_local");
    assert_eq!(job.command, "gitlab-ci-local --job build --job test");
    let ex = job.failure_excerpt.unwrap();
    assert!(ex.contains("npm i -g gitlab-ci-local"), "{ex}");
    assert_eq!(ex, gitlab_ci_local::INSTALL_HINT);
    let log = std::fs::read_to_string(&job.log_path).unwrap();
    assert!(log.contains("not_installed"), "{log}");
    let updates = job_updates(&drain(&mut rx));
    assert_eq!(updates.last().unwrap().status, CiStatus::NotInstalled);

    // no jobs listed => skipped, not run
    let job = sh_runner(&e)
        .with_env("PATH", empty_bin.to_string_lossy())
        .run_one(RUN, &e.worktree, &glc_recipe(&[]), &ctx(&[]))
        .await
        .unwrap();
    assert_eq!(job.status, CiStatus::Skipped);
}

/// With the real PATH of this machine: gitlab-ci-local is not installed here (SPEC hard rule 4), so
/// the job must be NotInstalled — and in any case it must not panic.
#[tokio::test]
async fn gitlab_ci_local_real_path_never_crashes() {
    let e = env();
    let job = sh_runner(&e)
        .run_one(RUN, &e.worktree, &glc_recipe(&["build"]), &ctx(&[]))
        .await
        .unwrap();
    if which::which("gitlab-ci-local").is_err() {
        assert!(
            matches!(
                job.status,
                CiStatus::NotInstalled | CiStatus::Failed | CiStatus::Passed
            ),
            "{job:?}"
        );
    }
}

#[tokio::test]
async fn gitlab_ci_local_runs_each_job_when_installed() {
    let e = env();
    let bin = e.base.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    write_exe(
        &bin.join("gitlab-ci-local"),
        "#!/bin/sh\n# fake: gitlab-ci-local --job <name>\necho \"running job $2\"\n[ \"$2\" = broken ] && { echo 'Error: job failed'; exit 1; }\nexit 0\n",
    );
    let path = format!("{}:/usr/bin:/bin", bin.display());

    let job = sh_runner(&e)
        .with_env("PATH", path.clone())
        .run_one(RUN, &e.worktree, &glc_recipe(&["build"]), &ctx(&[]))
        .await
        .unwrap();
    assert_eq!(job.status, CiStatus::Passed, "{job:?}");
    assert!(
        job.command.ends_with("gitlab-ci-local --job build"),
        "{}",
        job.command
    );
    assert!(std::fs::read_to_string(&job.log_path)
        .unwrap()
        .contains("running job build"));

    let job = sh_runner(&e)
        .with_env("PATH", path)
        .run_one(
            RUN,
            &e.worktree,
            &glc_recipe(&["broken", "build"]),
            &ctx(&[]),
        )
        .await
        .unwrap();
    assert_eq!(job.status, CiStatus::Failed);
    let log = std::fs::read_to_string(&job.log_path).unwrap();
    assert!(
        log.contains("running job broken") && log.contains("running job build"),
        "{log}"
    );
    assert!(job.failure_excerpt.unwrap().contains("Error: job failed"));
}

/// The production default shell (`/bin/zsh -lc` on macOS) works with the clean environment.
#[tokio::test]
async fn default_shell_runs_commands() {
    let e = env();
    let runner = CiRunner::new(e.bus.clone(), &e.logs);
    if Path::new("/bin/zsh").exists() {
        assert_eq!(
            runner.shell,
            vec!["/bin/zsh".to_string(), "-lc".to_string()]
        );
    }
    let mut r = recipe("zsh", "echo from-default-shell");
    r.timeout = Some("60s".into());
    let job = runner
        .run_one(RUN, &e.worktree, &r, &ctx(&[]))
        .await
        .unwrap();
    assert_eq!(job.status, CiStatus::Passed, "{:?}", job.failure_excerpt);
    assert!(std::fs::read_to_string(&job.log_path)
        .unwrap()
        .contains("from-default-shell"));
}
