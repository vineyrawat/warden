//! Read-only smoke test of the add-project flow against a real forge, using the token Warden saved
//! in the macOS Keychain. Makes GET requests only; never writes to the forge.
//!
//!   cargo run -p warden-core --example live_discover -- https://gitlab.example.com [group/project]
use warden_core::discovery;
use warden_core::forge::{ForgeProvider, GitlabProvider};
use warden_core::{secrets, ForgeKind};

#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    let base = args
        .next()
        .expect("base url")
        .trim_end_matches('/')
        .to_string();
    let probe = args.next();
    let token = secrets::get_token(&secrets::service_key(ForgeKind::Gitlab, &base))
        .expect("keychain")
        .expect("no token saved for this host");
    let gl = GitlabProvider::new(&base, token);
    println!("user: @{}", gl.whoami().await.expect("whoami"));

    let t = std::time::Instant::now();
    let remote = gl.list_project_details("").await.expect("list projects");
    println!("{} projects in {:?}", remote.len(), t.elapsed());
    for p in remote.iter().take(8) {
        println!(
            "  {:<45} default={:<10} clone={}",
            p.remote_id,
            p.default_branch.as_deref().unwrap_or("-"),
            p.clone_url.is_some()
        );
    }

    let roots = discovery::default_roots();
    let scanned = discovery::scan_repos(&roots);
    let d = discovery::discovery(&base, remote.clone(), &roots, scanned, &[]);
    let matched: Vec<_> = d
        .projects
        .iter()
        .filter(|p| !p.local_paths.is_empty())
        .collect();
    println!(
        "{} local repos scanned, {} projects matched to a clone:",
        d.local_repos.len(),
        matched.len()
    );
    for p in matched.iter().take(15) {
        println!("  {:<45} {}", p.remote.remote_id, p.local_paths.join(" | "));
    }
    let dups = d
        .local_repos
        .iter()
        .filter(|r| r.duplicate_of.is_some())
        .count();
    let other = d
        .local_repos
        .iter()
        .filter(|r| r.remote_id.is_none())
        .count();
    println!("skipped: {dups} duplicates, {other} not on this forge");

    let target = probe.or_else(|| matched.first().map(|p| p.remote.remote_id.clone()));
    if let Some(id) = target {
        let prot = gl
            .protected_branches(&id)
            .await
            .expect("protected branches");
        let def = remote
            .iter()
            .find(|r| r.remote_id == id)
            .and_then(|r| r.default_branch.clone());
        println!(
            "{id}: protected={prot:?} -> target branches {:?}",
            discovery::target_branches(def.as_deref(), &prot)
        );
    }
}
