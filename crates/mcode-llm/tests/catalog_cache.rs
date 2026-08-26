//! Local HTTP coverage for per-provider Pi catalog refresh and cache.

#[expect(
    dead_code,
    reason = "this catalog test uses only the GET subset of shared HTTP helpers"
)]
mod common;

use std::time::Duration;

use common::{MockResponse, MockServer};
use mcode_llm::{
    AuthProfile, CATALOG_CONTEXT_CLAMP_TOKENS, CatalogClient, CatalogOrigin, CatalogRefresh,
    ClientIdentity, ModelCatalog, ProfileProvider, ProviderProfile, WireKind,
};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

fn object_map_catalog(context: u64) -> Value {
    json!({
        "model-a": {
            "id": "model-a",
            "name": "Model A",
            "provider": "sample",
            "reasoning": true,
            "tool_call": true,
            "contextWindow": context,
            "maxTokens": 10,
            "input": ["text"],
            "cost": {"input": 1, "output": 2, "cacheRead": 0, "cacheWrite": 0},
            "baseUrl": "https://evil.example/injected",
            "headers": {"authorization": "Bearer injected-secret", "x-evil": "1"},
            "auth": {"scheme": "bearer", "env": "EVIL_KEY"}
        }
    })
}

fn array_catalog(context: u64) -> Value {
    json!([{
        "id": "model-a",
        "name": "Model A",
        "contextWindow": context,
        "maxTokens": 10,
        "reasoning": true,
        "input": ["text"]
    }])
}

fn catalog_client(dir: &std::path::Path, base: &str) -> CatalogClient {
    CatalogClient::new(dir)
        .with_base_url(base)
        .unwrap()
        .with_timeout(Duration::from_secs(2))
        .with_attempt_timeout(Duration::from_millis(400))
        .with_max_attempts(2)
        .with_identity(ClientIdentity::pi("linux", "6.8", "x64").unwrap())
}

#[tokio::test]
async fn object_map_and_array_200_write_independent_provider_cache() {
    let directory = tempfile::tempdir().unwrap();
    let first = MockResponse::json(
        "200 OK",
        &[
            ("ETag", "\"v1\""),
            ("Last-Modified", "Wed, 21 Oct 2015 07:28:00 GMT"),
        ],
        object_map_catalog(2_000),
    );
    let second = MockResponse::json("200 OK", &[("ETag", "\"v2\"")], array_catalog(3_000));
    let mut server = MockServer::spawn(vec![first, second]);
    let client = catalog_client(directory.path(), &server.base_url());

    let map = client.load("sample").await;
    assert_eq!(map.origin, CatalogOrigin::Network);
    assert_eq!(map.etag.as_deref(), Some("\"v1\""));
    assert_eq!(
        map.catalog
            .model("sample", "model-a")
            .unwrap()
            .settings
            .context_window,
        Some(2_000)
    );
    let request = server.request().await;
    assert_eq!(request.method, "GET");
    assert_eq!(request.path, "/api/models/providers/sample");
    assert_eq!(request.header("if-none-match"), None);
    assert_eq!(request.header("user-agent"), Some("pi (linux 6.8; x64)"));
    assert!(client.cache_path("sample").unwrap().is_file());

    let array = client.load("other").await;
    assert_eq!(array.origin, CatalogOrigin::Network);
    assert_eq!(
        array
            .catalog
            .model("other", "model-a")
            .unwrap()
            .settings
            .context_window,
        Some(3_000)
    );
    let request = server.request().await;
    assert_eq!(request.path, "/api/models/providers/other");
    assert!(client.cache_path("other").unwrap().is_file());
    assert_eq!(
        map.catalog
            .model("sample", "model-a")
            .unwrap()
            .settings
            .context_window,
        Some(2_000),
        "per-provider caches must not clobber each other"
    );
}

#[tokio::test]
async fn force_304_reuses_body_and_etag_without_body_never_sends_validator() {
    let directory = tempfile::tempdir().unwrap();
    let first = MockResponse::json(
        "200 OK",
        &[("ETag", "\"catalog-v1\"")],
        object_map_catalog(2_000),
    );
    let not_modified = MockResponse::chunks(
        "304 Not Modified",
        "application/json",
        &[("ETag", "\"catalog-v1\"")],
        Vec::new(),
        Duration::ZERO,
    );
    let missing = MockResponse::json("404 Not Found", &[], json!({"error": "missing"}));
    let empty_304 = MockResponse::chunks(
        "304 Not Modified",
        "application/json",
        &[("ETag", "\"dangling\"")],
        Vec::new(),
        Duration::ZERO,
    );
    let mut server = MockServer::spawn(vec![first, not_modified, missing, empty_304]);
    let client = catalog_client(directory.path(), &server.base_url());

    assert_eq!(client.load("sample").await.origin, CatalogOrigin::Network);
    let _ = server.request().await;

    let reused = client.refresh("sample", CatalogRefresh::force()).await;
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
    let second = server.request().await;
    assert_eq!(second.header("if-none-match"), Some("\"catalog-v1\""));

    let empty_dir = tempfile::tempdir().unwrap();
    let empty_client = catalog_client(empty_dir.path(), &server.base_url());
    assert_eq!(
        empty_client.load("sample").await.origin,
        CatalogOrigin::Unsupported
    );
    let _ = server.request().await;
    let path = empty_client.cache_path("sample").unwrap();
    let mut envelope: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    envelope["etag"] = json!("\"dangling\"");
    std::fs::write(&path, envelope.to_string()).unwrap();
    let snapshot = empty_client
        .refresh("sample", CatalogRefresh::force())
        .await;
    assert_eq!(snapshot.origin, CatalogOrigin::Unsupported);
    assert!(snapshot.catalog.model("sample", "model-a").is_none());
    assert!(snapshot.catalog.model("openai", "gpt-4o-mini").is_some());
    let third = server.request().await;
    assert_eq!(
        third.header("if-none-match"),
        None,
        "an ETag without a cached body must not issue If-None-Match"
    );
}

#[tokio::test]
async fn not_found_and_not_implemented_are_cached_without_retry_inside_window() {
    let directory = tempfile::tempdir().unwrap();
    let mut server = MockServer::spawn(vec![
        MockResponse::json("404 Not Found", &[], json!({"error": "missing"})),
        MockResponse::json("501 Not Implemented", &[], json!({"error": "nope"})),
        MockResponse::json("200 OK", &[], object_map_catalog(9)),
    ]);
    let client = catalog_client(directory.path(), &server.base_url());

    let missing = client.load("sample").await;
    assert_eq!(missing.origin, CatalogOrigin::Unsupported);
    assert!(missing.catalog.model("sample", "model-a").is_none());
    let _ = server.request().await;

    let again = client.load("sample").await;
    assert_eq!(again.origin, CatalogOrigin::Unsupported);
    assert!(
        tokio::time::timeout(Duration::from_millis(80), server.request())
            .await
            .is_err(),
        "fresh 404 cache must not revalidate inside the default window"
    );

    let unimplemented = client.load("other").await;
    assert_eq!(unimplemented.origin, CatalogOrigin::Unsupported);
    let request = server.request().await;
    assert_eq!(request.path, "/api/models/providers/other");
}

#[tokio::test]
async fn offline_and_refresh_failure_use_last_valid_cache() {
    let directory = tempfile::tempdir().unwrap();
    let mut seed_server = MockServer::spawn(vec![
        MockResponse::json("200 OK", &[("ETag", "\"seed\"")], object_map_catalog(3_000)),
        MockResponse::json("503 Service Unavailable", &[], json!({"error":"offline"})),
    ]);
    let seed_client = catalog_client(directory.path(), &seed_server.base_url());
    assert_eq!(
        seed_client.load("sample").await.origin,
        CatalogOrigin::Network
    );
    let _ = seed_server.request().await;

    let offline = CatalogClient::new(directory.path())
        .with_base_url(seed_server.base_url())
        .unwrap()
        .with_offline(true)
        .load("sample")
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

    let stale = CatalogClient::new(directory.path())
        .with_base_url(seed_server.base_url())
        .unwrap()
        .with_refresh_interval(Duration::ZERO)
        .with_max_attempts(1)
        .with_timeout(Duration::from_millis(400))
        .load("sample")
        .await;
    assert_eq!(stale.origin, CatalogOrigin::Cache);
    assert!(stale.catalog.model("sample", "model-a").is_some());
    let request = seed_server.request().await;
    assert_eq!(request.header("if-none-match"), Some("\"seed\""));
}

#[tokio::test]
async fn service_unavailable_refresh_advances_checked_at_without_changing_models() {
    let directory = tempfile::tempdir().unwrap();
    let mut server = MockServer::spawn(vec![
        MockResponse::json("200 OK", &[("ETag", "\"seed\"")], object_map_catalog(3_000)),
        MockResponse::json("503 Service Unavailable", &[], json!({"error": "offline"})),
    ]);
    let client = catalog_client(directory.path(), &server.base_url()).with_max_attempts(1);

    let first = client.load("sample").await;
    assert_eq!(first.origin, CatalogOrigin::Network);
    assert_eq!(first.etag.as_deref(), Some("\"seed\""));
    let _ = server.request().await;

    let path = client.cache_path("sample").unwrap();
    let mut envelope: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    envelope["checked_at"] = json!(1_u64);
    std::fs::write(&path, envelope.to_string()).unwrap();

    let stale = client.refresh("sample", CatalogRefresh::force()).await;
    assert_eq!(stale.origin, CatalogOrigin::Cache);
    assert_eq!(stale.etag.as_deref(), Some("\"seed\""));
    assert_eq!(
        stale
            .catalog
            .model("sample", "model-a")
            .unwrap()
            .settings
            .context_window,
        Some(3_000)
    );
    let checked_at = stale
        .checked_at
        .expect("503 refresh must report checked_at");
    assert!(
        checked_at > 1,
        "503 refresh must persist and return a newer checked_at"
    );
    let disk: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(disk["checked_at"], json!(checked_at));
    assert_eq!(disk["etag"], json!("\"seed\""));
    let _ = server.request().await;
}

#[tokio::test]
async fn corrupt_cache_and_bad_refresh_fall_back_without_blocking() {
    let directory = tempfile::tempdir().unwrap();
    let mut server = MockServer::spawn(vec![MockResponse::json(
        "200 OK",
        &[],
        json!({"not":"a provider catalog"}),
    )]);
    let client = catalog_client(directory.path(), &server.base_url())
        .with_timeout(Duration::from_millis(250));
    std::fs::write(
        client.cache_path("sample").unwrap(),
        b"{ definitely corrupt",
    )
    .unwrap();
    let snapshot = client.load("sample").await;
    assert_eq!(snapshot.origin, CatalogOrigin::BuiltInFallback);
    assert!(snapshot.catalog.model("openai", "gpt-4o-mini").is_some());
    assert!(
        ModelCatalog::from_provider_json(
            "sample",
            std::fs::read(client.cache_path("sample").unwrap()).unwrap()
        )
        .is_err()
    );
    let request = server.request().await;
    assert_eq!(request.header("if-none-match"), None);
}

#[tokio::test]
async fn oversize_slow_and_cancel_use_fallback() {
    let directory = tempfile::tempdir().unwrap();
    let huge = vec![b'x'; 1_024 * 1_024 + 64];
    let oversize = MockResponse::chunks(
        "200 OK",
        "application/json",
        &[],
        vec![huge],
        Duration::ZERO,
    );
    let slow = MockResponse::chunks(
        "200 OK",
        "application/json",
        &[],
        vec![br#"{"id":"late"}"#.to_vec()],
        Duration::from_secs(2),
    );
    let stall = MockResponse::stall();
    let mut server = MockServer::spawn(vec![oversize, slow, stall]);
    let base = server.base_url();

    let oversized = CatalogClient::new(directory.path())
        .with_base_url(&base)
        .unwrap()
        .with_timeout(Duration::from_secs(1))
        .with_max_attempts(1)
        .load("sample")
        .await;
    assert_eq!(oversized.origin, CatalogOrigin::BuiltInFallback);
    let _ = server.request().await;

    let slow_shot = CatalogClient::new(directory.path())
        .with_base_url(&base)
        .unwrap()
        .with_timeout(Duration::from_millis(150))
        .with_attempt_timeout(Duration::from_millis(80))
        .with_max_attempts(1)
        .load("sample")
        .await;
    assert_eq!(slow_shot.origin, CatalogOrigin::BuiltInFallback);
    let _ = server.request().await;

    let cancel = CancellationToken::new();
    cancel.cancel();
    let cancelled = CatalogClient::new(directory.path())
        .with_base_url(&base)
        .unwrap()
        .refresh(
            "sample",
            CatalogRefresh {
                force: true,
                allow_network: true,
                cancel,
            },
        )
        .await;
    assert_eq!(cancelled.origin, CatalogOrigin::BuiltInFallback);
}

#[tokio::test]
async fn cross_origin_redirect_is_not_followed() {
    let sink = MockServer::spawn(vec![MockResponse::json(
        "200 OK",
        &[],
        object_map_catalog(99),
    )]);
    let location = format!("{}/api/models/providers/sample", sink.base_url());
    let redirect = MockResponse::json(
        "307 Temporary Redirect",
        &[("Location", location.as_str())],
        json!({"error": "redirect blocked"}),
    );
    let mut source = MockServer::spawn(vec![redirect]);
    let directory = tempfile::tempdir().unwrap();
    let snapshot = catalog_client(directory.path(), &source.base_url())
        .load("sample")
        .await;
    assert_eq!(snapshot.origin, CatalogOrigin::BuiltInFallback);
    let _ = source.request().await;
    assert!(
        tokio::time::timeout(
            Duration::from_millis(100),
            sink.wait_for_completed_connections(1)
        )
        .await
        .is_err(),
        "cross-origin catalog redirect was followed"
    );
}

#[tokio::test]
async fn fresh_cache_skips_network_stale_revalidates_and_singleflight_shares_one_request() {
    let directory = tempfile::tempdir().unwrap();
    let first = MockResponse::json("200 OK", &[("ETag", "\"one\"")], object_map_catalog(4_000));
    let delayed = MockResponse::chunks(
        "200 OK",
        "application/json",
        &[("ETag", "\"two\"")],
        vec![object_map_catalog(5_000).to_string().into_bytes()],
        Duration::from_millis(80),
    );
    let stale = MockResponse::json(
        "200 OK",
        &[("ETag", "\"three\"")],
        object_map_catalog(6_000),
    );
    let mut server = MockServer::spawn(vec![first, delayed, stale]);
    let client = catalog_client(directory.path(), &server.base_url());

    assert_eq!(client.load("sample").await.origin, CatalogOrigin::Network);
    let _ = server.request().await;

    let fresh = client.load("sample").await;
    assert_eq!(fresh.origin, CatalogOrigin::Cache);
    assert!(
        tokio::time::timeout(Duration::from_millis(80), server.request())
            .await
            .is_err(),
        "fresh cache must not revalidate inside the default 4h window"
    );

    let other_dir = tempfile::tempdir().unwrap();
    let racing = CatalogClient::new(other_dir.path())
        .with_base_url(server.base_url())
        .unwrap()
        .with_timeout(Duration::from_secs(2))
        .with_max_attempts(1);
    let first_task = {
        let racing = racing.clone();
        tokio::spawn(async move { racing.load("sample").await })
    };
    let second_task = {
        let racing = racing.clone();
        tokio::spawn(async move { racing.load("sample").await })
    };
    let (left, right) = tokio::join!(first_task, second_task);
    assert_eq!(left.unwrap().origin, CatalogOrigin::Network);
    assert_eq!(right.unwrap().origin, CatalogOrigin::Network);
    let _ = server.request().await;
    assert!(
        tokio::time::timeout(Duration::from_millis(80), server.request())
            .await
            .is_err(),
        "singleflight must not issue a second catalog GET"
    );

    let stale_client = CatalogClient::new(directory.path())
        .with_base_url(server.base_url())
        .unwrap()
        .with_refresh_interval(Duration::ZERO)
        .with_timeout(Duration::from_secs(2));
    let updated = stale_client.load("sample").await;
    assert_eq!(updated.origin, CatalogOrigin::Network);
    assert_eq!(
        updated
            .catalog
            .model("sample", "model-a")
            .unwrap()
            .settings
            .context_window,
        Some(6_000)
    );
    let _ = server.request().await;
}

#[tokio::test]
async fn remote_endpoint_injection_does_not_change_profile_trust_domain() {
    let directory = tempfile::tempdir().unwrap();
    let mut server = MockServer::spawn(vec![MockResponse::json(
        "200 OK",
        &[],
        json!({
            "model-a": {
                "id": "model-a",
                "name": "Injected",
                "provider": "sample",
                "contextWindow": 1_000_000,
                "maxTokens": 99,
                "reasoning": true,
                "baseUrl": "https://evil.example/v1",
                "headers": {"authorization": "Bearer stolen", "x-api-key": "stolen"},
                "auth": {"scheme": "bearer", "env": "STOLEN"},
                "wire": "https://evil.example/messages"
            }
        }),
    )]);
    let snapshot = catalog_client(directory.path(), &server.base_url())
        .load("sample")
        .await;
    let _ = server.request().await;
    let model = snapshot.catalog.model("sample", "model-a").unwrap();
    assert_eq!(
        model.settings.context_window,
        Some(CATALOG_CONTEXT_CLAMP_TOKENS)
    );
    assert!(model.settings.headers.is_empty());
    assert!(model.wire_hint.is_none());

    let profile = ProviderProfile::new(
        "sample",
        WireKind::OpenAiChatCompletions,
        "http://127.0.0.1:9/v1",
        AuthProfile::none(),
    )
    .unwrap();
    let original_endpoint = profile.endpoint();
    let original_domain = profile.replay_domain();
    let provider = ProfileProvider::without_auth(profile)
        .unwrap()
        .with_catalog_settings(model.settings.clone());
    assert_eq!(provider.endpoint(), original_endpoint);
    assert_eq!(provider.profile().base_url(), "http://127.0.0.1:9/v1");
    assert_eq!(provider.profile().replay_domain(), original_domain);
    assert!(
        !provider.endpoint().contains("evil.example"),
        "remote baseUrl must not become the wire endpoint"
    );
}

#[tokio::test]
async fn ensure_fetched_skips_network_when_cache_exists_and_failure_keeps_explicit_model() {
    let directory = tempfile::tempdir().unwrap();
    let mut server = MockServer::spawn(vec![
        MockResponse::json(
            "200 OK",
            &[("ETag", "\"first\"")],
            object_map_catalog(8_000),
        ),
        MockResponse::json("200 OK", &[], object_map_catalog(1)),
    ]);
    let client = catalog_client(directory.path(), &server.base_url());
    assert_eq!(
        client.ensure_fetched("sample").await.origin,
        CatalogOrigin::Network
    );
    let _ = server.request().await;
    let cached = client.ensure_fetched("sample").await;
    assert_eq!(cached.origin, CatalogOrigin::Cache);
    assert!(
        tokio::time::timeout(Duration::from_millis(80), server.request())
            .await
            .is_err()
    );

    let missing = tempfile::tempdir().unwrap();
    let mut down = MockServer::spawn(vec![MockResponse::json(
        "503 Service Unavailable",
        &[],
        json!({"error":"down"}),
    )]);
    let fallback = catalog_client(missing.path(), &down.base_url())
        .with_max_attempts(1)
        .ensure_fetched("sample")
        .await;
    assert_eq!(fallback.origin, CatalogOrigin::BuiltInFallback);
    assert!(fallback.catalog.model("openai", "gpt-4o-mini").is_some());
    let _ = down.request().await;
}

#[tokio::test]
async fn load_lazy_fetches_missing_cache_and_spawns_stale_refresh() {
    let directory = tempfile::tempdir().unwrap();
    let first = MockResponse::json("200 OK", &[("ETag", "\"lazy\"")], object_map_catalog(7_000));
    let refresh = MockResponse::json(
        "200 OK",
        &[("ETag", "\"later\"")],
        object_map_catalog(7_100),
    );
    let mut server = MockServer::spawn(vec![first, refresh]);
    let client = CatalogClient::new(directory.path())
        .with_base_url(server.base_url())
        .unwrap()
        .with_refresh_interval(Duration::ZERO)
        .with_timeout(Duration::from_secs(2));

    let missing = client.load_lazy("sample").await;
    assert_eq!(missing.origin, CatalogOrigin::Network);
    let _ = server.request().await;

    let stale = client.load_lazy("sample").await;
    assert_eq!(stale.origin, CatalogOrigin::Cache);
    let later = server.request().await;
    assert_eq!(later.path, "/api/models/providers/sample");
}

#[tokio::test]
async fn user_json_overrides_still_win_after_remote_overlay() {
    let directory = tempfile::tempdir().unwrap();
    let mut server = MockServer::spawn(vec![MockResponse::json(
        "200 OK",
        &[],
        object_map_catalog(2_000),
    )]);
    let snapshot = catalog_client(directory.path(), &server.base_url())
        .load("sample")
        .await;
    let _ = server.request().await;
    let catalog = snapshot
        .catalog
        .model("sample", "model-a")
        .unwrap()
        .settings
        .clone();
    let user = mcode_llm::ModelSettings {
        context_window: Some(12_000),
        ..mcode_llm::ModelSettings::default()
    };
    let resolved = mcode_llm::resolve_model_settings(mcode_llm::ModelLayers {
        catalog: Some(&catalog),
        provider_config: Some(&user),
        ..mcode_llm::ModelLayers::default()
    });
    assert_eq!(resolved.context_window, Some(12_000));
}

#[tokio::test]
async fn catalog_origin_isolates_cache_etag_and_singleflight() {
    let directory = tempfile::tempdir().unwrap();
    let delayed = MockResponse::chunks(
        "200 OK",
        "application/json",
        &[("ETag", "\"custom\"")],
        vec![object_map_catalog(2_000).to_string().into_bytes()],
        Duration::from_millis(80),
    );
    let mut custom_server = MockServer::spawn(vec![
        delayed,
        MockResponse::json(
            "200 OK",
            &[("ETag", "\"custom-2\"")],
            object_map_catalog(2_100),
        ),
    ]);
    let mut other_server = MockServer::spawn(vec![MockResponse::json(
        "200 OK",
        &[("ETag", "\"other\"")],
        object_map_catalog(9_000),
    )]);
    let custom = catalog_client(directory.path(), &custom_server.base_url());
    let other = custom
        .clone()
        .with_base_url(other_server.base_url())
        .unwrap();

    let custom_task = {
        let custom = custom.clone();
        tokio::spawn(async move { custom.load("sample").await })
    };
    custom_server.wait_for_response_head().await;
    let other_shot = other.ensure_fetched("sample").await;
    assert_eq!(other_shot.origin, CatalogOrigin::Network);
    assert_eq!(
        other_shot
            .catalog
            .model("sample", "model-a")
            .unwrap()
            .settings
            .context_window,
        Some(9_000)
    );
    let other_request = other_server.request().await;
    assert_eq!(
        other_request.header("if-none-match"),
        None,
        "a different origin must not send another catalog's ETag"
    );
    let custom_shot = custom_task.await.unwrap();
    assert_eq!(custom_shot.origin, CatalogOrigin::Network);
    assert_eq!(
        custom_shot
            .catalog
            .model("sample", "model-a")
            .unwrap()
            .settings
            .context_window,
        Some(2_000)
    );
    let _ = custom_server.request().await;

    let reused = custom.ensure_fetched("sample").await;
    assert_eq!(reused.origin, CatalogOrigin::Cache);
    assert_eq!(
        reused
            .catalog
            .model("sample", "model-a")
            .unwrap()
            .settings
            .context_window,
        Some(2_000),
        "ensure_fetched must not return another origin's cache"
    );
    assert!(custom.cache_path("sample").unwrap().is_file());
    assert!(other.cache_path("sample").unwrap().is_file());
    assert_ne!(
        custom.cache_path("sample").unwrap(),
        other.cache_path("sample").unwrap()
    );
}

#[tokio::test(flavor = "current_thread")]
async fn sequential_force_refresh_does_not_reuse_completed_inflight() {
    let directory = tempfile::tempdir().unwrap();
    let first = MockResponse::json("200 OK", &[("ETag", "\"one\"")], object_map_catalog(2_000));
    let second = MockResponse::json("200 OK", &[("ETag", "\"two\"")], object_map_catalog(3_000));
    let mut server = MockServer::spawn(vec![first, second]);
    let client = catalog_client(directory.path(), &server.base_url());

    let first_shot = client.refresh("sample", CatalogRefresh::force()).await;
    assert_eq!(first_shot.origin, CatalogOrigin::Network);
    assert_eq!(
        first_shot
            .catalog
            .model("sample", "model-a")
            .unwrap()
            .settings
            .context_window,
        Some(2_000)
    );
    let _ = server.request().await;

    let second_shot = client.refresh("sample", CatalogRefresh::force()).await;
    assert_eq!(second_shot.origin, CatalogOrigin::Network);
    assert_eq!(
        second_shot
            .catalog
            .model("sample", "model-a")
            .unwrap()
            .settings
            .context_window,
        Some(3_000),
        "a force refresh after the previous flight returned must not reuse that snapshot"
    );
    let _ = server.request().await;
}

#[tokio::test]
async fn leader_cancel_during_fetch_does_not_poison_waiters() {
    let directory = tempfile::tempdir().unwrap();
    let delayed = MockResponse::chunks(
        "200 OK",
        "application/json",
        &[("ETag", "\"live\"")],
        vec![object_map_catalog(5_000).to_string().into_bytes()],
        Duration::from_millis(150),
    );
    let mut server = MockServer::spawn(vec![delayed]);
    let client = catalog_client(directory.path(), &server.base_url());
    let leader_cancel = CancellationToken::new();
    let leader = {
        let client = client.clone();
        let cancel = leader_cancel.clone();
        tokio::spawn(async move {
            client
                .refresh(
                    "sample",
                    CatalogRefresh {
                        force: true,
                        allow_network: true,
                        cancel,
                    },
                )
                .await
        })
    };
    server.wait_for_response_head().await;
    let waiter = {
        let client = client.clone();
        tokio::spawn(async move { client.refresh("sample", CatalogRefresh::force()).await })
    };
    leader_cancel.cancel();
    let leader_shot = leader.await.unwrap();
    let waiter_shot = waiter.await.unwrap();
    assert_eq!(leader_shot.origin, CatalogOrigin::BuiltInFallback);
    assert_eq!(waiter_shot.origin, CatalogOrigin::Network);
    assert_eq!(
        waiter_shot
            .catalog
            .model("sample", "model-a")
            .unwrap()
            .settings
            .context_window,
        Some(5_000)
    );
    let _ = server.request().await;
    assert!(
        tokio::time::timeout(Duration::from_millis(80), server.request())
            .await
            .is_err(),
        "cancelled leader must not force waiters to start a second GET"
    );
}

#[tokio::test]
async fn body_timeout_retries_remaining_attempts() {
    let directory = tempfile::tempdir().unwrap();
    let slow = MockResponse::chunks(
        "200 OK",
        "application/json",
        &[],
        vec![object_map_catalog(2_000).to_string().into_bytes()],
        Duration::from_millis(400),
    );
    let fast = MockResponse::json(
        "200 OK",
        &[("ETag", "\"retry\"")],
        object_map_catalog(2_000),
    );
    let mut server = MockServer::spawn(vec![slow, fast]);
    let snapshot = CatalogClient::new(directory.path())
        .with_base_url(server.base_url())
        .unwrap()
        .with_timeout(Duration::from_secs(2))
        .with_attempt_timeout(Duration::from_millis(80))
        .with_max_attempts(2)
        .load("sample")
        .await;
    assert_eq!(snapshot.origin, CatalogOrigin::Network);
    assert_eq!(
        snapshot
            .catalog
            .model("sample", "model-a")
            .unwrap()
            .settings
            .context_window,
        Some(2_000)
    );
    let _ = server.request().await;
    let _ = server.request().await;
}

// Rust guideline compliant 2026-08-26
