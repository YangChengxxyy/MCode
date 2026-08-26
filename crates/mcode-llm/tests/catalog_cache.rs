//! Local HTTP coverage for catalog ETag, cache, offline, and fallback behavior.

#[expect(
    dead_code,
    reason = "this catalog test uses only the GET subset of shared HTTP helpers"
)]
mod common;

use std::time::Duration;

use common::{MockResponse, MockServer};
use mcode_llm::{CatalogClient, CatalogOrigin, ClientIdentity, ModelCatalog};
use serde_json::{Value, json};

fn catalog_json(context: u64) -> Value {
    json!({
        "sample": {
            "id": "sample",
            "name": "Sample",
            "models": {
                "model-a": {
                    "id": "model-a",
                    "name": "Model A",
                    "reasoning": true,
                    "tool_call": true,
                    "attachment": false,
                    "limit": {"context": context, "input": context - 10, "output": 10},
                    "modalities": {"input": ["text"], "output": ["text"]}
                }
            }
        }
    })
}

#[tokio::test]
async fn refresh_writes_cache_and_304_reuses_it_with_etag() {
    let directory = tempfile::tempdir().unwrap();
    let cache_path = directory.path().join("nested/catalog-cache.json");
    let first = MockResponse::json("200 OK", &[("ETag", "\"catalog-v1\"")], catalog_json(2_000));
    let not_modified = MockResponse::chunks(
        "304 Not Modified",
        "application/json",
        &[("ETag", "\"catalog-v1\"")],
        Vec::new(),
        Duration::ZERO,
    );
    let mut server = MockServer::spawn(vec![first, not_modified]);
    let client = CatalogClient::new(&cache_path)
        .with_endpoint(format!("{}/api.json", server.base_url()))
        .unwrap()
        .with_identity(ClientIdentity::pi("linux", "6.8", "x64").unwrap());

    let fresh = client.load().await;
    assert_eq!(fresh.origin, CatalogOrigin::Network);
    assert_eq!(fresh.etag.as_deref(), Some("\"catalog-v1\""));
    assert_eq!(
        fresh
            .catalog
            .model("sample", "model-a")
            .unwrap()
            .settings
            .context_window,
        Some(2_000)
    );
    assert!(cache_path.is_file());
    let first_request = server.request().await;
    assert_eq!(first_request.method, "GET");
    assert_eq!(first_request.path, "/api.json");
    assert_eq!(first_request.header("if-none-match"), None);
    assert_eq!(
        first_request.header("user-agent"),
        Some("pi (linux 6.8; x64)")
    );

    let reused = client.load().await;
    assert_eq!(reused.origin, CatalogOrigin::NotModified);
    assert_eq!(
        reused
            .catalog
            .model("sample", "model-a")
            .unwrap()
            .settings
            .context_window,
        Some(2_000)
    );
    let second_request = server.request().await;
    assert_eq!(
        second_request.header("if-none-match"),
        Some("\"catalog-v1\"")
    );
}

#[tokio::test]
async fn offline_and_refresh_failure_use_last_valid_cache() {
    let directory = tempfile::tempdir().unwrap();
    let cache_path = directory.path().join("catalog.json");
    // Seed a real cache through the public refresh path.
    let mut seed_server = MockServer::spawn(vec![MockResponse::json(
        "200 OK",
        &[("ETag", "\"seed\"")],
        catalog_json(3_000),
    )]);
    let seed_client = CatalogClient::new(&cache_path)
        .with_endpoint(format!("{}/api.json", seed_server.base_url()))
        .unwrap();
    assert_eq!(seed_client.load().await.origin, CatalogOrigin::Network);
    let _ = seed_server.request().await;

    let offline = CatalogClient::new(&cache_path)
        .with_offline(true)
        .load()
        .await;
    assert_eq!(offline.origin, CatalogOrigin::Cache);
    assert_eq!(offline.etag.as_deref(), Some("\"seed\""));
    assert_eq!(
        offline
            .catalog
            .model("sample", "model-a")
            .unwrap()
            .settings
            .context_window,
        Some(3_000)
    );

    let mut failing_server = MockServer::spawn(vec![MockResponse::json(
        "503 Service Unavailable",
        &[],
        json!({"error":"offline"}),
    )]);
    let stale = CatalogClient::new(&cache_path)
        .with_endpoint(format!("{}/api.json", failing_server.base_url()))
        .unwrap()
        .load()
        .await;
    assert_eq!(stale.origin, CatalogOrigin::Cache);
    assert!(stale.catalog.model("sample", "model-a").is_some());
    let request = failing_server.request().await;
    assert_eq!(request.header("if-none-match"), Some("\"seed\""));
}

#[tokio::test]
async fn corrupt_cache_and_bad_refresh_fall_back_without_blocking() {
    let directory = tempfile::tempdir().unwrap();
    let cache_path = directory.path().join("catalog.json");
    std::fs::write(&cache_path, b"{ definitely corrupt").unwrap();
    let mut server = MockServer::spawn(vec![MockResponse::json(
        "200 OK",
        &[],
        json!({"not":"a models.dev catalog"}),
    )]);
    let snapshot = CatalogClient::new(&cache_path)
        .with_endpoint(format!("{}/api.json", server.base_url()))
        .unwrap()
        .with_timeout(Duration::from_millis(250))
        .load()
        .await;
    assert_eq!(snapshot.origin, CatalogOrigin::BuiltInFallback);
    assert!(snapshot.catalog.model("openai", "gpt-4o-mini").is_some());
    assert!(ModelCatalog::from_models_dev(std::fs::read(&cache_path).unwrap()).is_err());
    let request = server.request().await;
    assert_eq!(request.header("if-none-match"), None);
}

// Rust guideline compliant 2026-08-26
