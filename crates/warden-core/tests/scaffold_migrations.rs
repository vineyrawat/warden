// OWNER: scaffold
//! Sanity check that the embedded migrations apply cleanly to an in-memory SQLite database.

use sqlx::sqlite::SqlitePoolOptions;
use sqlx::Row;

#[tokio::test]
async fn migrations_apply() {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    sqlx::migrate!("./migrations").run(&pool).await.unwrap();

    let rows = sqlx::query("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
        .fetch_all(&pool)
        .await
        .unwrap();
    let names: Vec<String> = rows.iter().map(|r| r.get::<String, _>("name")).collect();
    for t in [
        "settings",
        "projects",
        "project_overrides",
        "merge_requests",
        "review_runs",
        "findings",
        "ci_jobs",
        "audit_events",
    ] {
        assert!(names.iter().any(|n| n == t), "missing table {t}: {names:?}");
    }

    sqlx::query("INSERT INTO projects (forge, base_url, remote_id, name) VALUES ('gitlab', 'https://gitlab.example', 'g/p', 'p')")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO merge_requests (project_id, iid) VALUES (1, 5)")
        .execute(&pool)
        .await
        .unwrap();
    let dup = sqlx::query("INSERT INTO merge_requests (project_id, iid) VALUES (1, 5)")
        .execute(&pool)
        .await;
    assert!(dup.is_err(), "project_id + iid must be unique");
}
