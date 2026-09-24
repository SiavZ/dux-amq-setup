//! User journeys for the add-project dialog's "Check out the default branch
//! before adding" box, against a real router, a real engine, real SQLite and a
//! real git repository whose `origin` is a local bare repository.
//!
//! The repository's remote default (`origin/HEAD`) is `main`, but the working
//! copy sits on `feature`, which carries a commit `main` does not have. That
//! commit is how each journey tells which branch a new agent's worktree was
//! started from: the dialog promises "New worktrees will branch from ..." and
//! these tests hold it to that sentence.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use dux_core::config::{DuxPaths, ProviderCommandConfig};
use dux_core::storage::SessionStore;
use dux_web::bootstrap::bootstrap_engine;
use dux_web::engine_actor::spawn_engine_thread;
use dux_web::server::router;

fn git(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

struct Fixture {
    addr: SocketAddr,
    tmp: tempfile::TempDir,
    repo: PathBuf,
    /// The commit only `feature` has.
    feature_commit: String,
}

impl Fixture {
    fn db_path(&self) -> PathBuf {
        self.tmp.path().join("dux").join("sessions.sqlite3")
    }
}

/// A clone of a bare `origin` whose HEAD names `main`, checked out on `feature`
/// with one commit of its own, pushed so the pre-create pull has an upstream.
fn repo_on_a_feature_branch(root: &Path) -> (PathBuf, String) {
    let seed = root.join("seed");
    std::fs::create_dir_all(&seed).unwrap();
    git(&seed, &["init", "-q", "-b", "main"]);
    git(&seed, &["config", "user.email", "t@example.com"]);
    git(&seed, &["config", "user.name", "Test"]);
    std::fs::write(seed.join("README.md"), "base\n").unwrap();
    git(&seed, &["add", "README.md"]);
    git(&seed, &["commit", "-q", "-m", "base"]);

    let origin = root.join("origin.git");
    git(
        root,
        &[
            "clone",
            "-q",
            "--bare",
            seed.to_string_lossy().as_ref(),
            origin.to_string_lossy().as_ref(),
        ],
    );

    let repo = root.join("repo");
    git(
        root,
        &[
            "clone",
            "-q",
            origin.to_string_lossy().as_ref(),
            repo.to_string_lossy().as_ref(),
        ],
    );
    git(&repo, &["config", "user.email", "t@example.com"]);
    git(&repo, &["config", "user.name", "Test"]);
    // A clone records origin/HEAD, which is how dux KNOWS the default branch.
    assert_eq!(
        git(&repo, &["symbolic-ref", "refs/remotes/origin/HEAD"]),
        "refs/remotes/origin/main"
    );
    git(&repo, &["switch", "-q", "-c", "feature"]);
    std::fs::write(repo.join("feature.txt"), "only on feature\n").unwrap();
    git(&repo, &["add", "feature.txt"]);
    git(&repo, &["commit", "-q", "-m", "feature work"]);
    git(&repo, &["push", "-q", "-u", "origin", "feature"]);
    let feature_commit = git(&repo, &["rev-parse", "HEAD"]);
    (repo, feature_commit)
}

async fn boot() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let (repo, feature_commit) = repo_on_a_feature_branch(tmp.path());

    let state = tmp.path().join("dux");
    let paths = DuxPaths {
        root: state.clone(),
        config_path: state.join("config.toml"),
        sessions_db_path: state.join("sessions.sqlite3"),
        worktrees_root: state.join("worktrees"),
        lock_path: state.join("dux.lock"),
    };
    std::fs::create_dir_all(&paths.worktrees_root).unwrap();
    let mut engine = bootstrap_engine(&paths).unwrap();
    dux_core::test_provider::defuse_config(&mut engine.config);
    // The agent CLI is the one thing that cannot run here; `cat` stands in for
    // it so the create journey spawns a real PTY.
    engine.config.providers.commands.insert(
        "claude".to_string(),
        ProviderCommandConfig {
            command: "cat".to_string(),
            args: vec![],
            resume_args: None,
            ..Default::default()
        },
    );
    let (handle, _join) = spawn_engine_thread(engine);
    let app = router(handle);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    Fixture {
        addr,
        tmp,
        repo,
        feature_commit,
    }
}

/// Add the repository through the real route and return the new project's id,
/// waiting out the checkout worker when the box was ticked.
async fn add_project(f: &Fixture, checkout_default: bool) -> String {
    let resp = reqwest::Client::new()
        .post(format!("http://{}/api/v1/projects", f.addr))
        .json(&serde_json::json!({
            "path": f.repo.to_string_lossy(),
            "name": "repo",
            "checkout_default": checkout_default,
        }))
        .send()
        .await
        .unwrap();
    let status = resp.status();
    assert!(status.is_success(), "add must succeed, got {status}");
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    loop {
        let projects: serde_json::Value =
            reqwest::get(format!("http://{}/api/v1/projects", f.addr))
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
        if let Some(project) = projects.as_array().and_then(|list| list.first()) {
            return project["id"].as_str().unwrap().to_string();
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the project never appeared"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Create an agent in the project through the real route and return its
/// worktree path once the engine has one on disk.
async fn create_agent(f: &Fixture, project_id: &str) -> PathBuf {
    let resp = reqwest::Client::new()
        .post(format!("http://{}/api/v1/sessions", f.addr))
        .json(&serde_json::json!({"kind": "new", "project_id": project_id, "name": "agent-one"}))
        .send()
        .await
        .unwrap();
    let status = resp.status();
    assert!(
        status.is_success(),
        "create must succeed, got {status}: {}",
        resp.text().await.unwrap_or_default()
    );
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        let sessions: serde_json::Value =
            reqwest::get(format!("http://{}/api/v1/sessions", f.addr))
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
        let worktree = sessions.as_array().and_then(|list| {
            list.iter().find_map(|s| {
                s["workspace"]["worktree_path"]
                    .as_str()
                    .map(PathBuf::from)
                    .filter(|path| path.join(".git").exists())
            })
        });
        if let Some(worktree) = worktree {
            return worktree;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the agent's worktree never appeared: {sessions}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn contains_commit(worktree: &Path, commit: &str) -> bool {
    std::process::Command::new("git")
        .args(["merge-base", "--is-ancestor", commit, "HEAD"])
        .current_dir(worktree)
        .status()
        .unwrap()
        .success()
}

fn stored_base(f: &Fixture, project_id: &str) -> Option<String> {
    SessionStore::open(&f.db_path())
        .unwrap()
        .load_projects()
        .unwrap()
        .into_iter()
        .find(|p| p.id == project_id)
        .and_then(|p| p.leading_branch)
}

#[tokio::test]
async fn adding_without_the_checkout_branches_new_worktrees_from_the_current_branch() {
    let f = boot().await;

    let project_id = add_project(&f, false).await;

    assert_eq!(
        stored_base(&f, &project_id).as_deref(),
        Some("feature"),
        "an unticked box records the branch the folder is on as the project's base"
    );
    let worktree = create_agent(&f, &project_id).await;
    assert!(
        contains_commit(&worktree, &f.feature_commit),
        "the dialog said new worktrees branch from \"feature\", so the worktree must carry its commit"
    );
    assert_eq!(
        git(&f.repo, &["symbolic-ref", "--short", "HEAD"]),
        "feature",
        "the user's folder stays where it was"
    );
}

#[tokio::test]
async fn adding_with_the_checkout_branches_new_worktrees_from_the_default_branch() {
    let f = boot().await;

    let project_id = add_project(&f, true).await;

    assert_eq!(
        git(&f.repo, &["symbolic-ref", "--short", "HEAD"]),
        "main",
        "a ticked box checks the default branch out in the user's folder"
    );
    assert_eq!(stored_base(&f, &project_id).as_deref(), Some("main"));
    let worktree = create_agent(&f, &project_id).await;
    assert!(
        !contains_commit(&worktree, &f.feature_commit),
        "new worktrees branch from \"main\", which does not have the feature commit"
    );
}

#[tokio::test]
async fn checking_out_the_default_branch_later_moves_the_project_base_to_it() {
    let f = boot().await;
    let project_id = add_project(&f, false).await;
    assert_eq!(stored_base(&f, &project_id).as_deref(), Some("feature"));

    let resp = reqwest::Client::new()
        .post(format!(
            "http://{}/api/v1/projects/{project_id}/checkout-default",
            f.addr
        ))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success(), "got {}", resp.status());

    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while stored_base(&f, &project_id).as_deref() != Some("main") {
        assert!(
            std::time::Instant::now() < deadline,
            "the project's base never moved to the default branch"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(git(&f.repo, &["symbolic-ref", "--short", "HEAD"]), "main");
    let worktree = create_agent(&f, &project_id).await;
    assert!(
        !contains_commit(&worktree, &f.feature_commit),
        "after checking out the default branch, new worktrees branch from it"
    );
}
