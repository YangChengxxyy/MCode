//! OAuth 2.1/PKCE and static-secret assembly through host ports.

// Rust guideline compliant 2026-08-20.

use std::sync::Arc;

use async_trait::async_trait;
use http::HeaderName;
use rmcp::transport::auth::{
    AuthError as RmcpAuthError, AuthorizationManager, AuthorizationRequest, AuthorizationSession,
    CredentialStore, OAuthClientConfig, OAuthHttpClient, OAuthHttpClientError,
    OAuthHttpClientFuture, OAuthHttpRedirectPolicy, OAuthHttpRequest, StateStore,
    StoredAuthorizationState, StoredCredentials,
};
use serde_json::Value;
use url::Url;

use crate::{
    config::{AuthConfig, OAuth2Config, OAuthRegistration, StreamableHttpTransportConfig},
    error::{Error, ErrorKind, Recovery, Result},
    host::{AuthHostHandle, AuthorizationPresentation},
    http::{
        BearerTokenProvider, DnsResolverHandle, HttpSecurityPolicy, RedirectMode, SecureHttpClient,
        SecureHttpError,
    },
    identity::ServerName,
    secret::{SecretBytes, SecretStoreKey, SecretValue},
};

const MAX_OAUTH_RESPONSE_BYTES: usize = 1024 * 1024;

/// Builds a secure MCP HTTP client without exposing credentials to settings JSON.
pub(crate) async fn build_http_client(
    server: &ServerName,
    transport: &StreamableHttpTransportConfig,
    auth_host: &AuthHostHandle,
    resolver: DnsResolverHandle,
    policy: HttpSecurityPolicy,
) -> Result<SecureHttpClient> {
    let base = SecureHttpClient::new(policy, resolver);
    let mut secret_headers = Vec::with_capacity(transport.headers.len());
    for (name, binding) in &transport.headers {
        let name = HeaderName::from_bytes(name.as_bytes())
            .map_err(|_| auth_error(server, "configured secret header name is invalid"))?;
        let value = auth_host
            .inner()
            .resolve_secret(server, &binding.secret_ref)
            .await?;
        secret_headers.push((name, value));
    }

    let client = match &transport.auth {
        AuthConfig::None => base,
        AuthConfig::StaticBearer { secret_ref } => {
            let bearer = auth_host.inner().resolve_secret(server, secret_ref).await?;
            base.with_bearer(bearer)
        }
        AuthConfig::OAuth2(config) => {
            let provider =
                authorize_oauth(server, &transport.url, config, auth_host, &base).await?;
            base.with_bearer_provider(provider)
        }
    };
    Ok(client.with_secret_headers(secret_headers))
}

async fn authorize_oauth(
    server: &ServerName,
    endpoint: &str,
    config: &OAuth2Config,
    host: &AuthHostHandle,
    http: &SecureHttpClient,
) -> Result<OAuthBearerProvider> {
    let oauth_http = Arc::new(SecureOAuthHttpClient {
        inner: http.clone(),
    });
    let mut manager = AuthorizationManager::new_with_oauth_http_client(endpoint, oauth_http)
        .await
        .map_err(|_| auth_error(server, "OAuth manager initialization failed"))?;
    let credential_store = HostCredentialStore::new(server.clone(), host.clone())?;
    manager.set_credential_store(credential_store.clone());
    manager.set_state_store(HostStateStore::new(server.clone(), host.clone())?);
    manager.set_allow_missing_issuer(false);

    if manager
        .initialize_from_store()
        .await
        .map_err(|_| auth_error(server, "stored OAuth credentials could not be initialized"))?
    {
        if stored_oauth_identity_matches(server, config, &manager).await? {
            // Confidential pre-registered material must be attached before the first
            // token read or refresh. Restoring after TokenRefreshFailed is too late.
            restore_oauth_client_config(server, config, host, &mut manager).await?;
            match manager.get_access_token().await {
                Ok(_) => {
                    return Ok(OAuthBearerProvider::new(
                        server.clone(),
                        config.redirect_uri.clone(),
                        config.scopes.clone(),
                        host.clone(),
                        manager,
                        credential_store,
                    ));
                }
                Err(error) if requires_reauthorization(&error) => {}
                Err(error) => return Err(oauth_token_error(server, &error)),
            }
        }
        credential_store.clear().await.map_err(|_| {
            auth_error(
                server,
                "unusable stored OAuth credentials could not be cleared",
            )
        })?;
    }

    let resolution = manager
        .resolve_metadata()
        .await
        .map_err(|_| auth_error(server, "OAuth metadata discovery failed"))?;
    if !resolution.source.is_discovered() {
        return Err(auth_error(
            server,
            "OAuth authorization metadata was not published by the server",
        ));
    }
    match &config.registration {
        OAuthRegistration::ClientMetadata { .. }
            if resolution
                .metadata
                .additional_fields
                .get("client_id_metadata_document_supported")
                .and_then(Value::as_bool)
                != Some(true) =>
        {
            return Err(auth_error(
                server,
                "authorization server did not advertise Client ID Metadata Documents",
            ));
        }
        OAuthRegistration::Dynamic { .. }
            if resolution.metadata.registration_endpoint.is_none() =>
        {
            return Err(auth_error(
                server,
                "authorization server did not advertise Dynamic Client Registration",
            ));
        }
        _ => {}
    }
    manager.set_metadata(resolution.metadata);

    let request = authorization_request(server, config, host).await?;
    let session = AuthorizationSession::new(manager, request)
        .await
        .map_err(|(_, _)| auth_error(server, "OAuth client registration or PKCE setup failed"))?;
    let callback = host
        .inner()
        .authorize_browser(AuthorizationPresentation {
            server: server.clone(),
            authorization_url: session.get_authorization_url().to_owned(),
            redirect_uri: config.redirect_uri.clone(),
        })
        .await?;
    session
        .handle_callback_url(&callback.redirect_url)
        .await
        .map_err(|_| auth_error(server, "OAuth callback validation or token exchange failed"))?;
    let provider = OAuthBearerProvider::new(
        server.clone(),
        config.redirect_uri.clone(),
        config.scopes.clone(),
        host.clone(),
        session.auth_manager,
        credential_store,
    );
    let _ = provider.token().await.map_err(Error::from)?;
    Ok(provider)
}

#[derive(Clone)]
struct OAuthBearerProvider {
    server: ServerName,
    redirect_uri: String,
    scopes: Vec<String>,
    host: AuthHostHandle,
    manager: Arc<tokio::sync::Mutex<AuthorizationManager>>,
    credential_store: HostCredentialStore,
    interactive_authorization: Arc<tokio::sync::Mutex<()>>,
}

impl OAuthBearerProvider {
    fn new(
        server: ServerName,
        redirect_uri: String,
        scopes: Vec<String>,
        host: AuthHostHandle,
        manager: AuthorizationManager,
        credential_store: HostCredentialStore,
    ) -> Self {
        Self {
            server,
            redirect_uri,
            scopes,
            host,
            manager: Arc::new(tokio::sync::Mutex::new(manager)),
            credential_store,
            interactive_authorization: Arc::new(tokio::sync::Mutex::new(())),
        }
    }

    fn secure_error(&self, message: impl AsRef<str>) -> SecureHttpError {
        SecureHttpError::Authentication(auth_error(&self.server, message))
    }

    async fn reauthorize(&self) -> std::result::Result<SecretValue, SecureHttpError> {
        self.credential_store
            .clear()
            .await
            .map_err(|_| self.secure_error("unusable OAuth credentials could not be cleared"))?;
        let authorization_url = {
            let manager = self.manager.lock().await;
            let defaults: Vec<&str> = self.scopes.iter().map(String::as_str).collect();
            let scopes = manager.select_scopes(None, &defaults);
            let scopes: Vec<&str> = scopes.iter().map(String::as_str).collect();
            manager
                .get_authorization_url(&scopes)
                .await
                .map_err(|_| self.secure_error("OAuth reauthorization could not start"))?
        };
        let callback = self
            .host
            .inner()
            .authorize_browser(AuthorizationPresentation {
                server: self.server.clone(),
                authorization_url,
                redirect_uri: self.redirect_uri.clone(),
            })
            .await
            .map_err(SecureHttpError::Authentication)?;
        let callback =
            rmcp::transport::auth::AuthorizationCallback::from_redirect_url(&callback.redirect_url)
                .map_err(|_| self.secure_error("OAuth reauthorization callback was invalid"))?;
        let manager = self.manager.lock().await;
        manager
            .exchange_code_for_token_with_issuer(
                &callback.code,
                &callback.csrf_token,
                callback.issuer.as_deref(),
            )
            .await
            .map_err(|_| self.secure_error("OAuth reauthorization token exchange failed"))?;
        manager
            .get_access_token()
            .await
            .map(SecretValue::new)
            .map_err(|error| {
                SecureHttpError::Authentication(oauth_token_error(&self.server, &error))
            })
    }
}

#[async_trait]
impl BearerTokenProvider for OAuthBearerProvider {
    async fn token(&self) -> std::result::Result<SecretValue, SecureHttpError> {
        let _authorization = self.interactive_authorization.lock().await;
        let token = self.manager.lock().await.get_access_token().await;
        match token {
            Ok(token) => Ok(SecretValue::new(token)),
            Err(error) if requires_reauthorization(&error) => self.reauthorize().await,
            Err(error) => Err(SecureHttpError::Authentication(oauth_token_error(
                &self.server,
                &error,
            ))),
        }
    }

    async fn upgrade_scope(
        &self,
        required_scope: &str,
    ) -> std::result::Result<bool, SecureHttpError> {
        let _authorization = self.interactive_authorization.lock().await;
        let authorization_url = self
            .manager
            .lock()
            .await
            .request_scope_upgrade(required_scope)
            .await
            .map_err(|_| self.secure_error("OAuth scope upgrade could not start"))?;
        let callback = self
            .host
            .inner()
            .authorize_browser(AuthorizationPresentation {
                server: self.server.clone(),
                authorization_url,
                redirect_uri: self.redirect_uri.clone(),
            })
            .await
            .map_err(SecureHttpError::Authentication)?;
        let callback =
            rmcp::transport::auth::AuthorizationCallback::from_redirect_url(&callback.redirect_url)
                .map_err(|_| self.secure_error("OAuth scope callback was invalid"))?;
        self.manager
            .lock()
            .await
            .exchange_code_for_token_with_issuer(
                &callback.code,
                &callback.csrf_token,
                callback.issuer.as_deref(),
            )
            .await
            .map_err(|_| self.secure_error("OAuth scope token exchange failed"))?;
        Ok(true)
    }
}

/// Returns whether stored credentials belong to the configured pre-registered client.
async fn stored_oauth_identity_matches(
    server: &ServerName,
    config: &OAuth2Config,
    manager: &AuthorizationManager,
) -> Result<bool> {
    let OAuthRegistration::PreRegistered { client_id, .. } = &config.registration else {
        return Ok(true);
    };
    let (stored_id, _) = manager
        .get_credentials()
        .await
        .map_err(|_| auth_error(server, "stored OAuth client identity is unavailable"))?;
    Ok(stored_id == *client_id)
}

/// Restores redirect, scopes, and confidential pre-registered secret material.
///
/// Pre-registered credentials must already match the configured client ID. The
/// secret is copied only into the SDK client and is never logged or stored in
/// ordinary settings JSON. Callers must invoke this before the first token read.
async fn restore_oauth_client_config(
    server: &ServerName,
    config: &OAuth2Config,
    host: &AuthHostHandle,
    manager: &mut AuthorizationManager,
) -> Result<()> {
    let client_id = match &config.registration {
        OAuthRegistration::PreRegistered { client_id, .. } => client_id.clone(),
        OAuthRegistration::Auto { .. }
        | OAuthRegistration::ClientMetadata { .. }
        | OAuthRegistration::Dynamic { .. } => {
            manager
                .get_credentials()
                .await
                .map_err(|_| auth_error(server, "stored OAuth client identity is unavailable"))?
                .0
        }
    };
    let mut restored = OAuthClientConfig::new(client_id, config.redirect_uri.clone())
        .with_scopes(config.scopes.clone());
    if let OAuthRegistration::PreRegistered {
        client_secret: Some(binding),
        ..
    } = &config.registration
    {
        let secret = host
            .inner()
            .resolve_secret(server, &binding.secret_ref)
            .await?;
        restored = restored.with_client_secret(secret.expose().to_owned());
    }
    manager
        .configure_client(restored)
        .map_err(|_| auth_error(server, "stored OAuth client could not be restored"))
}

async fn authorization_request(
    server: &ServerName,
    config: &OAuth2Config,
    host: &AuthHostHandle,
) -> Result<AuthorizationRequest> {
    let mut request =
        AuthorizationRequest::new(config.redirect_uri.clone()).with_scopes(config.scopes.clone());
    request = match &config.registration {
        OAuthRegistration::Auto {
            client_name,
            client_metadata_url,
        } => {
            let request = request.with_client_name(client_name.clone());
            match client_metadata_url {
                Some(url) => request.with_client_metadata_url(url.clone()),
                None => request,
            }
        }
        OAuthRegistration::PreRegistered {
            client_id,
            client_secret,
        } => {
            let mut request = request.with_preregistered_client(client_id.clone());
            if let Some(binding) = client_secret {
                let secret = host
                    .inner()
                    .resolve_secret(server, &binding.secret_ref)
                    .await?;
                request = request.with_client_secret(secret.expose().to_owned());
            }
            request
        }
        OAuthRegistration::ClientMetadata { url } => request.with_client_metadata_url(url.clone()),
        OAuthRegistration::Dynamic { client_name } => request.with_client_name(client_name.clone()),
    };
    Ok(request)
}

#[derive(Clone)]
struct HostCredentialStore {
    host: AuthHostHandle,
    key: SecretStoreKey,
}

impl HostCredentialStore {
    fn new(server: ServerName, host: AuthHostHandle) -> Result<Self> {
        Ok(Self {
            host,
            key: SecretStoreKey::new(format!("mcode.mcp/{server}/oauth/credentials"))?,
        })
    }
}

#[async_trait]
impl CredentialStore for HostCredentialStore {
    async fn load(&self) -> std::result::Result<Option<StoredCredentials>, RmcpAuthError> {
        let value = self
            .host
            .inner()
            .load_record(&self.key)
            .await
            .map_err(|_| store_error())?;
        value
            .map(|value| serde_json::from_slice(value.expose_secret()).map_err(|_| store_error()))
            .transpose()
    }

    async fn save(&self, credentials: StoredCredentials) -> std::result::Result<(), RmcpAuthError> {
        let value = serde_json::to_vec(&credentials).map_err(|_| store_error())?;
        self.host
            .inner()
            .save_record(&self.key, SecretBytes::new(value))
            .await
            .map_err(|_| store_error())
    }

    async fn clear(&self) -> std::result::Result<(), RmcpAuthError> {
        self.host
            .inner()
            .delete_record(&self.key)
            .await
            .map_err(|_| store_error())
    }
}

#[derive(Clone)]
struct HostStateStore {
    server: ServerName,
    host: AuthHostHandle,
}

impl HostStateStore {
    fn new(server: ServerName, host: AuthHostHandle) -> Result<Self> {
        let _ = SecretStoreKey::new(format!("mcode.mcp/{server}/oauth/state"))?;
        Ok(Self { server, host })
    }

    fn key(&self, csrf_token: &str) -> std::result::Result<SecretStoreKey, RmcpAuthError> {
        if csrf_token.len() > 512 || !csrf_token.bytes().all(|byte| byte.is_ascii_graphic()) {
            return Err(store_error());
        }
        SecretStoreKey::new(format!(
            "mcode.mcp/{}/oauth/state/{csrf_token}",
            self.server
        ))
        .map_err(|_| store_error())
    }
}

#[async_trait]
impl StateStore for HostStateStore {
    async fn save(
        &self,
        csrf_token: &str,
        state: StoredAuthorizationState,
    ) -> std::result::Result<(), RmcpAuthError> {
        let value = serde_json::to_vec(&state).map_err(|_| store_error())?;
        self.host
            .inner()
            .save_record(&self.key(csrf_token)?, SecretBytes::new(value))
            .await
            .map_err(|_| store_error())
    }

    async fn load(
        &self,
        csrf_token: &str,
    ) -> std::result::Result<Option<StoredAuthorizationState>, RmcpAuthError> {
        self.host
            .inner()
            .load_record(&self.key(csrf_token)?)
            .await
            .map_err(|_| store_error())?
            .map(|value| serde_json::from_slice(value.expose_secret()).map_err(|_| store_error()))
            .transpose()
    }

    async fn delete(&self, csrf_token: &str) -> std::result::Result<(), RmcpAuthError> {
        self.host
            .inner()
            .delete_record(&self.key(csrf_token)?)
            .await
            .map_err(|_| store_error())
    }
}

#[derive(Clone)]
struct SecureOAuthHttpClient {
    inner: SecureHttpClient,
}

impl OAuthHttpClient for SecureOAuthHttpClient {
    fn execute(&self, operation: OAuthHttpRequest) -> OAuthHttpClientFuture<'_> {
        Box::pin(async move {
            let OAuthHttpRequest {
                request,
                redirect_policy,
                timeout,
                ..
            } = operation;
            let (parts, body) = request.into_parts();
            let url = Url::parse(&parts.uri.to_string())
                .map_err(|error| Box::new(error) as OAuthHttpClientError)?;
            let mode = match redirect_policy {
                OAuthHttpRedirectPolicy::Follow => RedirectMode::Follow,
                OAuthHttpRedirectPolicy::Stop => RedirectMode::Stop,
                _ => RedirectMode::Stop,
            };
            let response = self
                .inner
                .execute(parts.method, url, parts.headers, body, mode, timeout)
                .await
                .map_err(|error| Box::new(error) as OAuthHttpClientError)?;
            let (status, headers, body) = self
                .inner
                .read_body(response, MAX_OAUTH_RESPONSE_BYTES)
                .await
                .map_err(|error| Box::new(error) as OAuthHttpClientError)?;
            let mut builder = http::Response::builder().status(status);
            for (name, value) in &headers {
                builder = builder.header(name, value);
            }
            builder
                .body(body)
                .map_err(|error| Box::new(error) as OAuthHttpClientError)
        })
    }
}

fn store_error() -> RmcpAuthError {
    RmcpAuthError::InternalError("secure OAuth record store operation failed".to_owned())
}

fn requires_reauthorization(error: &RmcpAuthError) -> bool {
    matches!(
        error,
        RmcpAuthError::AuthorizationRequired | RmcpAuthError::TokenRefreshRejected(_)
    )
}

fn oauth_token_error(server: &ServerName, error: &RmcpAuthError) -> Error {
    let recovery = if requires_reauthorization(error)
        || matches!(error, RmcpAuthError::TokenRefreshFailed(_))
    {
        Recovery::Recoverable
    } else {
        Recovery::Fatal
    };
    Error::new(
        ErrorKind::Authentication,
        recovery,
        "OAuth access token is unavailable",
    )
    .with_server(server.clone())
}

fn auth_error(server: &ServerName, message: impl AsRef<str>) -> Error {
    Error::new(ErrorKind::Authentication, Recovery::Fatal, message).with_server(server.clone())
}

impl From<SecureHttpError> for Error {
    fn from(value: SecureHttpError) -> Self {
        let recovery = match value {
            SecureHttpError::Authentication(error) => return error,
            SecureHttpError::Request(_) | SecureHttpError::Dns(_) => Recovery::Recoverable,
            _ => Recovery::Fatal,
        };
        Error::new(
            ErrorKind::Transport,
            recovery,
            "secure HTTP operation failed",
        )
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashMap,
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
    };

    use axum::{
        Router,
        extract::State,
        http::{Method, StatusCode, Uri, header::CONTENT_TYPE},
        response::{IntoResponse, Response},
    };
    use tokio::sync::Mutex;

    use super::*;
    use crate::{
        OutputLimits, SecretBinding, SecretRef, SystemDnsResolver, TimeoutConfig, TrustConfig,
        TrustLevel,
    };

    #[derive(Debug, Clone, Default)]
    struct RecordingAuthHost {
        records: Arc<Mutex<HashMap<String, SecretBytes>>>,
        browser_calls: Arc<AtomicUsize>,
        secret_calls: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl crate::host::AuthHost for RecordingAuthHost {
        async fn resolve_secret(
            &self,
            _server: &ServerName,
            _secret_ref: &SecretRef,
        ) -> Result<SecretValue> {
            self.secret_calls.fetch_add(1, Ordering::SeqCst);
            Ok(SecretValue::new("current-client-secret"))
        }

        async fn load_record(&self, key: &SecretStoreKey) -> Result<Option<SecretBytes>> {
            Ok(self.records.lock().await.get(key.as_str()).cloned())
        }

        async fn save_record(&self, key: &SecretStoreKey, value: SecretBytes) -> Result<()> {
            self.records
                .lock()
                .await
                .insert(key.as_str().to_owned(), value);
            Ok(())
        }

        async fn delete_record(&self, key: &SecretStoreKey) -> Result<()> {
            self.records.lock().await.remove(key.as_str());
            Ok(())
        }

        async fn authorize_browser(
            &self,
            request: AuthorizationPresentation,
        ) -> Result<crate::host::AuthorizationCallback> {
            self.browser_calls.fetch_add(1, Ordering::SeqCst);
            let url = Url::parse(&request.authorization_url).unwrap();
            let state = url
                .query_pairs()
                .find_map(|(name, value)| (name == "state").then(|| value.into_owned()))
                .unwrap();
            let redirect_uri = url
                .query_pairs()
                .find_map(|(name, value)| (name == "redirect_uri").then(|| value.into_owned()))
                .unwrap();
            assert_eq!(redirect_uri, request.redirect_uri);
            Ok(crate::host::AuthorizationCallback {
                redirect_url: format!("{}?code=test-code&state={state}", request.redirect_uri),
            })
        }
    }

    async fn oauth_endpoint(
        State(base): State<String>,
        method: Method,
        uri: Uri,
        body: axum::body::Bytes,
    ) -> Response {
        let path = uri.path();
        if method == Method::POST && path == "/token" {
            if String::from_utf8_lossy(&body).contains("grant_type=refresh_token") {
                return (
                    StatusCode::BAD_REQUEST,
                    [(CONTENT_TYPE, "application/json")],
                    serde_json::json!({"error": "invalid_grant"}).to_string(),
                )
                    .into_response();
            }
            return (
                StatusCode::OK,
                [(CONTENT_TYPE, "application/json")],
                serde_json::json!({
                    "access_token": "fresh-token",
                    "token_type": "bearer",
                    "expires_in": 3600,
                    "scope": "read"
                })
                .to_string(),
            )
                .into_response();
        }
        if path.contains("oauth-protected-resource") {
            return (
                StatusCode::OK,
                [(CONTENT_TYPE, "application/json")],
                serde_json::json!({
                    "resource": format!("{base}/mcp"),
                    "authorization_servers": [base]
                })
                .to_string(),
            )
                .into_response();
        }
        if path.contains("oauth-authorization-server") || path.contains("openid-configuration") {
            return (
                StatusCode::OK,
                [(CONTENT_TYPE, "application/json")],
                serde_json::json!({
                    "issuer": base,
                    "authorization_endpoint": format!("{base}/authorize"),
                    "token_endpoint": format!("{base}/token"),
                    "response_types_supported": ["code"],
                    "code_challenge_methods_supported": ["S256"],
                    "scopes_supported": ["read", "offline_access"]
                })
                .to_string(),
            )
                .into_response();
        }
        StatusCode::NOT_FOUND.into_response()
    }

    #[tokio::test]
    async fn unusable_stored_token_falls_back_to_current_preregistered_client() {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let base = format!("http://127.0.0.1:{}", address.port());
        let app = Router::new()
            .fallback(oauth_endpoint)
            .with_state(base.clone());
        let server_task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let server = ServerName::new("oauth-test").unwrap();
        let host = RecordingAuthHost::default();
        let credential_key = format!("mcode.mcp/{server}/oauth/credentials");
        let stale = serde_json::json!({
            "client_id": "stale-client",
            "token_response": {
                "access_token": "stale-token",
                "token_type": "bearer",
                "expires_in": 1
            },
            "granted_scopes": ["read"],
            "token_received_at": 0,
            "issuer": base
        });
        host.records.lock().await.insert(
            credential_key.clone(),
            SecretBytes::new(serde_json::to_vec(&stale).unwrap()),
        );
        let host_handle = AuthHostHandle::new(host.clone());
        let config = OAuth2Config {
            redirect_uri: "http://127.0.0.1:8765/callback".to_owned(),
            scopes: vec!["read".to_owned()],
            registration: OAuthRegistration::PreRegistered {
                client_id: "current-client".to_owned(),
                client_secret: Some(SecretBinding {
                    secret_ref: SecretRef::new("client-secret").unwrap(),
                }),
            },
        };
        let policy = HttpSecurityPolicy::new(
            server.clone(),
            TrustConfig {
                level: TrustLevel::Trusted,
                allow_http: true,
                allow_localhost: true,
                ..TrustConfig::default()
            },
            OutputLimits::default(),
            TimeoutConfig::default(),
        );
        let http = SecureHttpClient::new(policy, DnsResolverHandle::new(SystemDnsResolver));

        let provider = authorize_oauth(
            &server,
            &format!("{base}/mcp"),
            &config,
            &host_handle,
            &http,
        )
        .await
        .unwrap();
        assert_eq!(provider.token().await.unwrap().expose(), "fresh-token");
        assert_eq!(host.browser_calls.load(Ordering::SeqCst), 1);
        assert_eq!(host.secret_calls.load(Ordering::SeqCst), 1);
        let stored = host
            .records
            .lock()
            .await
            .get(&credential_key)
            .cloned()
            .unwrap();
        let stored: Value = serde_json::from_slice(stored.expose_secret()).unwrap();
        assert_eq!(stored["client_id"], "current-client");
        assert_ne!(stored["token_response"]["access_token"], "stale-token");

        server_task.abort();
    }

    #[tokio::test]
    async fn valid_stored_token_with_mismatched_preregistered_client_is_not_reused() {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let base = format!("http://127.0.0.1:{}", address.port());
        let app = Router::new()
            .fallback(oauth_endpoint)
            .with_state(base.clone());
        let server_task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let server = ServerName::new("oauth-valid-mismatch").unwrap();
        let host = RecordingAuthHost::default();
        let credential_key = format!("mcode.mcp/{server}/oauth/credentials");
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let valid = serde_json::json!({
            "client_id": "stale-client",
            "token_response": {
                "access_token": "stale-token",
                "token_type": "bearer",
                "expires_in": 3600
            },
            "granted_scopes": ["read"],
            "token_received_at": now,
            "issuer": base
        });
        host.records.lock().await.insert(
            credential_key.clone(),
            SecretBytes::new(serde_json::to_vec(&valid).unwrap()),
        );
        let host_handle = AuthHostHandle::new(host.clone());
        let config = OAuth2Config {
            redirect_uri: "http://127.0.0.1:8765/callback".to_owned(),
            scopes: vec!["read".to_owned()],
            registration: OAuthRegistration::PreRegistered {
                client_id: "current-client".to_owned(),
                client_secret: Some(SecretBinding {
                    secret_ref: SecretRef::new("client-secret").unwrap(),
                }),
            },
        };
        let policy = HttpSecurityPolicy::new(
            server.clone(),
            TrustConfig {
                level: TrustLevel::Trusted,
                allow_http: true,
                allow_localhost: true,
                ..TrustConfig::default()
            },
            OutputLimits::default(),
            TimeoutConfig::default(),
        );
        let http = SecureHttpClient::new(policy, DnsResolverHandle::new(SystemDnsResolver));

        let provider = authorize_oauth(
            &server,
            &format!("{base}/mcp"),
            &config,
            &host_handle,
            &http,
        )
        .await
        .unwrap();
        assert_eq!(provider.token().await.unwrap().expose(), "fresh-token");
        assert_eq!(host.browser_calls.load(Ordering::SeqCst), 1);
        assert_eq!(host.secret_calls.load(Ordering::SeqCst), 1);
        let stored = host
            .records
            .lock()
            .await
            .get(&credential_key)
            .cloned()
            .unwrap();
        let stored: Value = serde_json::from_slice(stored.expose_secret()).unwrap();
        assert_eq!(stored["client_id"], "current-client");
        assert_ne!(stored["token_response"]["access_token"], "stale-token");

        server_task.abort();
    }

    async fn invalid_client_refresh_endpoint(
        State(base): State<String>,
        method: Method,
        uri: Uri,
        body: axum::body::Bytes,
    ) -> Response {
        if method == Method::POST
            && uri.path() == "/token"
            && String::from_utf8_lossy(&body).contains("grant_type=refresh_token")
        {
            return (
                StatusCode::BAD_REQUEST,
                [(CONTENT_TYPE, "application/json")],
                serde_json::json!({"error": "invalid_client"}).to_string(),
            )
                .into_response();
        }
        oauth_endpoint(State(base), method, uri, body).await
    }

    #[tokio::test]
    async fn expired_mismatched_preregistered_identity_does_not_refresh() {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let base = format!("http://127.0.0.1:{}", address.port());
        let app = Router::new()
            .fallback(invalid_client_refresh_endpoint)
            .with_state(base.clone());
        let server_task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let server = ServerName::new("oauth-expired-mismatch").unwrap();
        let host = RecordingAuthHost::default();
        let credential_key = format!("mcode.mcp/{server}/oauth/credentials");
        let expired = serde_json::json!({
            "client_id": "stale-client",
            "token_response": {
                "access_token": "expired-token",
                "token_type": "bearer",
                "expires_in": 1,
                "refresh_token": "stored-refresh"
            },
            "granted_scopes": ["read"],
            "token_received_at": 0,
            "issuer": base
        });
        host.records.lock().await.insert(
            credential_key.clone(),
            SecretBytes::new(serde_json::to_vec(&expired).unwrap()),
        );
        let host_handle = AuthHostHandle::new(host.clone());
        let config = OAuth2Config {
            redirect_uri: "http://127.0.0.1:8765/callback".to_owned(),
            scopes: vec!["read".to_owned()],
            registration: OAuthRegistration::PreRegistered {
                client_id: "current-client".to_owned(),
                client_secret: Some(SecretBinding {
                    secret_ref: SecretRef::new("client-secret").unwrap(),
                }),
            },
        };
        let policy = HttpSecurityPolicy::new(
            server.clone(),
            TrustConfig {
                level: TrustLevel::Trusted,
                allow_http: true,
                allow_localhost: true,
                ..TrustConfig::default()
            },
            OutputLimits::default(),
            TimeoutConfig::default(),
        );
        let http = SecureHttpClient::new(policy, DnsResolverHandle::new(SystemDnsResolver));

        let provider = authorize_oauth(
            &server,
            &format!("{base}/mcp"),
            &config,
            &host_handle,
            &http,
        )
        .await
        .unwrap();
        assert_eq!(provider.token().await.unwrap().expose(), "fresh-token");
        assert_eq!(host.browser_calls.load(Ordering::SeqCst), 1);
        assert_eq!(host.secret_calls.load(Ordering::SeqCst), 1);
        let stored = host
            .records
            .lock()
            .await
            .get(&credential_key)
            .cloned()
            .unwrap();
        let stored: Value = serde_json::from_slice(stored.expose_secret()).unwrap();
        assert_eq!(stored["client_id"], "current-client");
        assert_ne!(stored["token_response"]["access_token"], "expired-token");

        server_task.abort();
    }

    #[tokio::test]
    async fn rejected_online_refresh_runs_browser_reauthorization() {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let base = format!("http://127.0.0.1:{}", address.port());
        let app = Router::new()
            .fallback(oauth_endpoint)
            .with_state(base.clone());
        let server_task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let server = ServerName::new("oauth-online-test").unwrap();
        let host = RecordingAuthHost::default();
        let host_handle = AuthHostHandle::new(host.clone());
        let credential_key = format!("mcode.mcp/{server}/oauth/credentials");
        let config = OAuth2Config {
            redirect_uri: "http://127.0.0.1:8765/callback".to_owned(),
            scopes: vec!["read".to_owned()],
            registration: OAuthRegistration::PreRegistered {
                client_id: "current-client".to_owned(),
                client_secret: None,
            },
        };
        let policy = HttpSecurityPolicy::new(
            server.clone(),
            TrustConfig {
                level: TrustLevel::Trusted,
                allow_http: true,
                allow_localhost: true,
                ..TrustConfig::default()
            },
            OutputLimits::default(),
            TimeoutConfig::default(),
        );
        let http = SecureHttpClient::new(policy, DnsResolverHandle::new(SystemDnsResolver));
        let initial_provider = authorize_oauth(
            &server,
            &format!("{base}/mcp"),
            &config,
            &host_handle,
            &http,
        )
        .await
        .unwrap();
        drop(initial_provider);
        assert_eq!(host.browser_calls.load(Ordering::SeqCst), 1);
        let provider = authorize_oauth(
            &server,
            &format!("{base}/mcp"),
            &config,
            &host_handle,
            &http,
        )
        .await
        .unwrap();
        assert_eq!(host.browser_calls.load(Ordering::SeqCst), 1);

        let rejected_refresh = serde_json::json!({
            "client_id": "current-client",
            "token_response": {
                "access_token": "expired-token",
                "token_type": "bearer",
                "expires_in": 1,
                "refresh_token": "rejected-refresh"
            },
            "granted_scopes": ["read"],
            "token_received_at": 0,
            "issuer": base
        });
        host.records.lock().await.insert(
            credential_key.clone(),
            SecretBytes::new(serde_json::to_vec(&rejected_refresh).unwrap()),
        );

        assert_eq!(provider.token().await.unwrap().expose(), "fresh-token");
        assert_eq!(host.browser_calls.load(Ordering::SeqCst), 2);
        let stored = host
            .records
            .lock()
            .await
            .get(&credential_key)
            .cloned()
            .unwrap();
        let stored: Value = serde_json::from_slice(stored.expose_secret()).unwrap();
        assert_eq!(stored["token_response"]["access_token"], "fresh-token");
        assert_ne!(
            stored["token_response"]["refresh_token"],
            "rejected-refresh"
        );

        server_task.abort();
    }

    async fn confidential_refresh_endpoint(
        State(base): State<String>,
        method: Method,
        uri: Uri,
        headers: axum::http::HeaderMap,
        body: axum::body::Bytes,
    ) -> Response {
        let path = uri.path();
        if method == Method::POST && path == "/token" {
            let body = String::from_utf8_lossy(&body);
            if body.contains("grant_type=refresh_token") {
                let has_http_basic = headers
                    .get(axum::http::header::AUTHORIZATION)
                    .and_then(|value| value.to_str().ok())
                    .is_some_and(|value| value.starts_with("Basic "));
                if has_http_basic || body.contains("client_secret=") {
                    return (
                        StatusCode::OK,
                        [(CONTENT_TYPE, "application/json")],
                        serde_json::json!({
                            "access_token": "refreshed-token",
                            "token_type": "bearer",
                            "expires_in": 3600,
                            "refresh_token": "rotated-refresh"
                        })
                        .to_string(),
                    )
                        .into_response();
                }
                return (
                    StatusCode::BAD_REQUEST,
                    [(CONTENT_TYPE, "application/json")],
                    serde_json::json!({"error": "invalid_client"}).to_string(),
                )
                    .into_response();
            }
            return (
                StatusCode::BAD_REQUEST,
                [(CONTENT_TYPE, "application/json")],
                serde_json::json!({"error": "authorization_code_not_expected"}).to_string(),
            )
                .into_response();
        }
        oauth_endpoint(State(base), method, uri, body).await
    }

    #[tokio::test]
    async fn confidential_preregistered_secret_is_restored_before_token_refresh() {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let base = format!("http://127.0.0.1:{}", address.port());
        let app = Router::new()
            .fallback(confidential_refresh_endpoint)
            .with_state(base.clone());
        let server_task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let server = ServerName::new("oauth-confidential").unwrap();
        let host = RecordingAuthHost::default();
        let credential_key = format!("mcode.mcp/{server}/oauth/credentials");
        let stored = serde_json::json!({
            "client_id": "current-client",
            "token_response": {
                "access_token": "expired-token",
                "token_type": "bearer",
                "expires_in": 1,
                "refresh_token": "stored-refresh"
            },
            "granted_scopes": ["read"],
            "token_received_at": 0,
            "issuer": base
        });
        host.records.lock().await.insert(
            credential_key.clone(),
            SecretBytes::new(serde_json::to_vec(&stored).unwrap()),
        );
        let host_handle = AuthHostHandle::new(host.clone());
        let config = OAuth2Config {
            redirect_uri: "http://127.0.0.1:8765/callback".to_owned(),
            scopes: vec!["read".to_owned()],
            registration: OAuthRegistration::PreRegistered {
                client_id: "current-client".to_owned(),
                client_secret: Some(SecretBinding {
                    secret_ref: SecretRef::new("client-secret").unwrap(),
                }),
            },
        };
        let secret_marker = "current-client-secret";
        let encoded = serde_json::to_string(&config).unwrap();
        assert!(!encoded.contains(secret_marker));
        assert!(!format!("{config:?}").contains(secret_marker));
        assert!(!format!("{:?}", SecretValue::new(secret_marker)).contains(secret_marker));

        let policy = HttpSecurityPolicy::new(
            server.clone(),
            TrustConfig {
                level: TrustLevel::Trusted,
                allow_http: true,
                allow_localhost: true,
                ..TrustConfig::default()
            },
            OutputLimits::default(),
            TimeoutConfig::default(),
        );
        let http = SecureHttpClient::new(policy, DnsResolverHandle::new(SystemDnsResolver));
        let provider = authorize_oauth(
            &server,
            &format!("{base}/mcp"),
            &config,
            &host_handle,
            &http,
        )
        .await
        .unwrap();
        assert_eq!(provider.token().await.unwrap().expose(), "refreshed-token");
        assert_eq!(host.browser_calls.load(Ordering::SeqCst), 0);
        assert_eq!(host.secret_calls.load(Ordering::SeqCst), 1);
        let stored = host
            .records
            .lock()
            .await
            .get(&credential_key)
            .cloned()
            .unwrap();
        let stored: Value = serde_json::from_slice(stored.expose_secret()).unwrap();
        assert_eq!(stored["token_response"]["access_token"], "refreshed-token");
        assert!(!stored.to_string().contains(secret_marker));

        server_task.abort();
    }
}
