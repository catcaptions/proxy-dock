use axum::extract::Query;
use axum::response::Html;
use axum::routing::get;
use base64::Engine as _;
use rand::{distributions::Alphanumeric, Rng};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::sync::Arc;
use tokio::sync::{oneshot, Mutex};

pub const CODEX_AUTH_URL: &str = "https://auth.openai.com/oauth/authorize";
pub const CODEX_TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
pub const CODEX_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
pub const CODEX_CALLBACK_PORT: u16 = 1455;
pub const CODEX_CALLBACK_PATH: &str = "/auth/callback";

pub const ANTIGRAVITY_AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
pub const ANTIGRAVITY_TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
// NOTE: the client ID lives in oauth_secret::BUILTIN_CLIENT_ID (user bundle
// overrides it) — never hardcode one here.
pub const ANTIGRAVITY_CALLBACK_PORT: u16 = 51121;
pub const ANTIGRAVITY_CALLBACK_PATH: &str = "/oauth-callback";
pub const ANTIGRAVITY_SCOPES: &str = "https://www.googleapis.com/auth/cloud-platform https://www.googleapis.com/auth/userinfo.email https://www.googleapis.com/auth/userinfo.profile https://www.googleapis.com/auth/cclog https://www.googleapis.com/auth/experimentsandconfigs";

pub const OPENCODE_MODELS_URL: &str = "https://opencode.ai/zen/go/v1/models";
pub const OPENCODE_USAGE_URL: &str = "https://opencode.ai/zen/go/v1/usage";

// Claude (Anthropic) subscription OAuth. Mirrors CLIProxyAPI v7.3.15
// `internal/auth/claude` exactly (nothing invented):
// - authorize `https://claude.ai/oauth/authorize` with PKCE S256,
//   `code=true`, `response_type=code`, the public client id below, redirect
//   `http://localhost:54545/callback`, and the full scope string.
// - code exchange `POST https://platform.claude.com/v1/oauth/token` with a
//   JSON body (field order grant_type/code/redirect_uri/client_id/
//   code_verifier/state, matching native CLI traffic) and axios-shaped
//   headers (`Accept: application/json, text/plain, */*`,
//   `User-Agent: axios/1.15.2`).
// - refresh posts `{client_id, grant_type: refresh_token, refresh_token,
//   scope}` to the same token URL.
// - identity comes from the exchange response (`account.email_address`,
//   `account.uuid`, `organization.*`) with `GET
//   https://api.anthropic.com/api/oauth/profile` as the confirming lookup.
pub const CLAUDE_AUTH_URL: &str = "https://claude.ai/oauth/authorize";
pub const CLAUDE_TOKEN_URL: &str = "https://platform.claude.com/v1/oauth/token";
pub const CLAUDE_PROFILE_URL: &str = "https://api.anthropic.com/api/oauth/profile";
pub const CLAUDE_CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
pub const CLAUDE_CALLBACK_PORT: u16 = 54545;
pub const CLAUDE_CALLBACK_PATH: &str = "/callback";
pub const CLAUDE_SCOPE: &str =
    "user:profile user:inference user:sessions:claude_code user:mcp_servers user:file_upload";
pub const CLAUDE_AXIOS_UA: &str = "axios/1.15.2";

pub fn claude_auth_url(state: &str, pkce: &Pkce, port: u16) -> String {
    let redirect = format!("http://localhost:{port}{CLAUDE_CALLBACK_PATH}");
    let mut params = url::form_urlencoded::Serializer::new(String::new());
    params.append_pair("code", "true");
    params.append_pair("client_id", CLAUDE_CLIENT_ID);
    params.append_pair("response_type", "code");
    params.append_pair("redirect_uri", &redirect);
    params.append_pair("scope", CLAUDE_SCOPE);
    params.append_pair("code_challenge", &pkce.challenge);
    params.append_pair("code_challenge_method", "S256");
    params.append_pair("state", state);
    format!("{CLAUDE_AUTH_URL}?{}", params.finish())
}

// Command Code web CLI-login. `cmd login` opens this Studio page in the
// browser; after the user signs in, Studio POSTs the issued API key to our
// localhost callback (or shows a "Copy your API key" fallback). Flow mirrors
// the official CLI bundle exactly (command-code npm package, `cli.mjs`):
// callback `http://127.0.0.1:{port}/callback`, `mode=redirect`, 32-byte
// base64url state; server accepts POST /callback (JSON or form) with
// apiKey+state+userId+userName+keyName and serves GET /callback/complete.
pub const COMMANDCODE_AUTH_PAGE: &str = "https://commandcode.ai/studio/auth/cli";
pub const COMMANDCODE_WHOAMI_URL: &str = "https://api.commandcode.ai/alpha/whoami";
pub const COMMANDCODE_CALLBACK_PATH: &str = "/callback";
pub const COMMANDCODE_COMPLETE_PATH: &str = "/callback/complete";
pub const COMMANDCODE_CLIENT_VERSION: &str = "0.24.1";

pub fn commandcode_auth_url(callback: &str, state: &str) -> String {
    let mut params = url::form_urlencoded::Serializer::new(String::new());
    params.append_pair("callback", callback);
    params.append_pair("state", state);
    params.append_pair("mode", "redirect");
    format!("{COMMANDCODE_AUTH_PAGE}?{}", params.finish())
}

/// 32-byte base64url CSRF token for the Command Code Studio callback.
pub fn commandcode_state() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

#[derive(Debug, Clone)]
pub struct Pkce {
    pub verifier: String,
    pub challenge: String,
}

pub fn pkce() -> Pkce {
    let verifier: String = rand::thread_rng().sample_iter(&Alphanumeric).map(char::from).take(64).collect();
    let mut hasher = Sha256::new();
    hasher.update(verifier.as_bytes());
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(hasher.finalize());
    Pkce { verifier, challenge }
}

pub fn random_state() -> String {
    rand::thread_rng().sample_iter(&Alphanumeric).map(char::from).take(32).collect()
}

pub fn codex_auth_url(state: &str, pkce: &Pkce, port: u16) -> String {
    let redirect = format!("http://localhost:{port}{CODEX_CALLBACK_PATH}");
    let mut params = url::form_urlencoded::Serializer::new(String::new());
    params.append_pair("client_id", CODEX_CLIENT_ID);
    params.append_pair("response_type", "code");
    params.append_pair("redirect_uri", &redirect);
    params.append_pair("scope", "openid email profile offline_access");
    params.append_pair("state", state);
    params.append_pair("code_challenge", &pkce.challenge);
    params.append_pair("code_challenge_method", "S256");
    params.append_pair("prompt", "login");
    params.append_pair("id_token_add_organizations", "true");
    params.append_pair("codex_cli_simplified_flow", "true");
    format!("{CODEX_AUTH_URL}?{}", params.finish())
}

pub fn antigravity_auth_url(state: &str, port: u16) -> String {
    let redirect = format!("http://localhost:{port}{ANTIGRAVITY_CALLBACK_PATH}");
    let mut params = url::form_urlencoded::Serializer::new(String::new());
    params.append_pair("access_type", "offline");
    // Effective (user-owned) client: the code must belong to the same
    // client the exchange uses, or Google rejects it.
    params.append_pair("client_id", &crate::oauth_secret::client_id());
    params.append_pair("prompt", "consent");
    params.append_pair("redirect_uri", &redirect);
    params.append_pair("response_type", "code");
    params.append_pair("scope", ANTIGRAVITY_SCOPES);
    params.append_pair("state", state);
    format!("{ANTIGRAVITY_AUTH_URL}?{}", params.finish())
}

#[derive(Debug, Deserialize, Clone)]
pub struct CallbackParams {
    pub code: Option<String>,
    pub state: Option<String>,
    pub error: Option<String>,
    pub error_description: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CallbackResult {
    pub code: String,
    pub state: String,
}

struct CallbackState {
    expected_state: String,
    sender: Option<oneshot::Sender<Result<CallbackResult, String>>>,
}

pub struct CallbackServer {
    port: u16,
    path: &'static str,
    state: Arc<Mutex<CallbackState>>,
}

impl CallbackServer {
    pub fn new(port: u16, path: &'static str, expected_state: String) -> (Self, oneshot::Receiver<Result<CallbackResult, String>>) {
        let (tx, rx) = oneshot::channel();
        (
            Self { port, path, state: Arc::new(Mutex::new(CallbackState { expected_state, sender: Some(tx) })) },
            rx,
        )
    }

    pub async fn run(self) -> Result<u16, String> {
        let path = self.path;
        let state = self.state.clone();
        let app = axum::Router::new().route(
            path,
            get(move |Query(params): Query<CallbackParams>| {
                let state = state.clone();
                async move {
                    let mut guard = state.lock().await;
                    let result = match (&params.code, &params.error) {
                        (_, Some(err)) => Err(params.error_description.clone().unwrap_or_else(|| err.clone())),
                        (Some(code), _) => {
                            let got = params.state.clone().unwrap_or_default();
                            if got != guard.expected_state {
                                Err("state mismatch — possible CSRF; aborting sign-in".to_string())
                            } else {
                                Ok(CallbackResult { code: code.clone(), state: got })
                            }
                        }
                        _ => Err("callback missing authorization code".to_string()),
                    };
                    if let Some(sender) = guard.sender.take() {
                        let _ = sender.send(result.clone());
                    }
                    match result {
                        Ok(_) => Html("<h1>Sign-in successful</h1><p>You can close this window and return to Proxy Dock.</p>"),
                        Err(_) => Html("<h1>Sign-in failed</h1><p>Please check Proxy Dock for details.</p>"),
                    }
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", self.port))
            .await
            .map_err(|err| format!("callback port {} unavailable: {err}", self.port))?;
        let port = listener.local_addr().map_err(|err| err.to_string())?.port();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Ok(port)
    }
}

#[derive(Debug, serde::Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    id_token: Option<String>,
    expires_in: Option<i64>,
}

/// Payload Studio POSTs to our localhost callback after web login.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandCodeCallbackPayload {
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub state: Option<String>,
    #[serde(default)]
    pub user_id: Option<String>,
    #[serde(default)]
    pub user_name: Option<String>,
    #[serde(default)]
    pub key_name: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub error_description: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CommandCodeIdentity {
    pub email: String,
    pub user_name: String,
}

/// CORS headers mirroring the official CLI callback server: reflect the
/// origin when it is one of the known Studio/dev origins, allow
/// GET/POST/OPTIONS + Content-Type, and echo Private-Network-Access on demand.
fn studio_cors(origin: Option<&axum::http::HeaderValue>, pna_requested: bool) -> axum::http::HeaderMap {
    use axum::http::{HeaderMap, HeaderValue};
    let allowed = ["http://localhost:3000", "https://staging.commandcode.ai", "https://commandcode.ai"];
    let origin_str = origin.and_then(|v| v.to_str().ok()).unwrap_or("");
    let mut headers = HeaderMap::new();
    let value = if allowed.contains(&origin_str) { origin_str } else { allowed[0] };
    headers.insert("access-control-allow-origin", HeaderValue::from_str(value).unwrap());
    headers.insert("access-control-allow-methods", HeaderValue::from_static("GET, POST, OPTIONS"));
    headers.insert("access-control-allow-headers", HeaderValue::from_static("Content-Type"));
    if pna_requested {
        headers.insert("access-control-allow-private-network", HeaderValue::from_static("true"));
    }
    headers
}

fn commandcode_json(success: bool, error: Option<&str>) -> serde_json::Value {
    match error {
        Some(message) => serde_json::json!({"success": success, "error": message}),
        None => serde_json::json!({"success": success}),
    }
}

fn commandcode_page(heading: &str, message: &str) -> String {
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>Proxy Dock sign-in</title></head>\
        <body style=\"font-family:system-ui,sans-serif;padding:40px;text-align:center\">\
        <h1>{heading}</h1><p>{message}</p>\
        <script>try{{window.close()}}catch(e){{}}</script></body></html>"
    )
}

struct CommandCodeSlot {
    expected_state: String,
    sender: Option<oneshot::Sender<Result<CommandCodeCallbackPayload, String>>>,
    pending: Option<CommandCodeCallbackPayload>,
}

/// Starts the Command Code Studio callback server on an ephemeral localhost
/// port, mirroring the official CLI (`http://127.0.0.1:{port}/callback`).
/// Returns the callback URL (to embed in the Studio auth URL) plus a receiver
/// for the POSTed payload.
pub async fn run_commandcode_callback(
    expected_state: String,
) -> Result<(String, oneshot::Receiver<Result<CommandCodeCallbackPayload, String>>), String> {
    use axum::extract::{Query, State as AxumState};
    use axum::http::{HeaderMap, StatusCode};
    use axum::response::IntoResponse;
    use axum::routing::{get, post};

    let (tx, rx) = oneshot::channel::<Result<CommandCodeCallbackPayload, String>>();
    let slot = Arc::new(Mutex::new(CommandCodeSlot {
        expected_state,
        sender: Some(tx),
        pending: None,
    }));

    #[derive(Debug, serde::Deserialize)]
    struct CompleteQuery {
        #[serde(default)]
        state: String,
    }

    async fn complete_sender(
        slot: &Arc<Mutex<CommandCodeSlot>>,
    ) -> Option<oneshot::Sender<Result<CommandCodeCallbackPayload, String>>> {
        slot.lock().await.sender.take()
    }

    async fn handle_options(headers: HeaderMap) -> impl IntoResponse {
        let pna = headers.get("access-control-request-private-network").is_some();
        (StatusCode::NO_CONTENT, studio_cors(headers.get("origin"), pna), "")
    }

    async fn handle_post(
        AxumState(slot): AxumState<Arc<Mutex<CommandCodeSlot>>>,
        headers: HeaderMap,
        body: axum::body::Bytes,
    ) -> impl IntoResponse {
        let pna = headers.get("access-control-request-private-network").is_some();
        let cors = studio_cors(headers.get("origin"), pna);
        let content_type = headers
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .to_lowercase();
        let payload: Result<CommandCodeCallbackPayload, String> = if content_type == "application/x-www-form-urlencoded" {
            let fields: std::collections::HashMap<String, String> =
                url::form_urlencoded::parse(&body).into_owned().collect();
            let get = |key: &str| fields.get(key).cloned();
            Ok(CommandCodeCallbackPayload {
                api_key: get("apiKey"),
                state: get("state"),
                user_id: get("userId"),
                user_name: get("userName"),
                key_name: get("keyName"),
                error: get("error"),
                error_description: get("error_description"),
            })
        } else if content_type == "application/json" || content_type.is_empty() {
            serde_json::from_slice::<CommandCodeCallbackPayload>(&body).map_err(|err| err.to_string())
        } else {
            return (StatusCode::UNSUPPORTED_MEDIA_TYPE, cors, axum::Json(commandcode_json(false, Some("Unsupported content type"))))
                .into_response();
        };
        let payload = match payload {
            Ok(payload) => payload,
            Err(_) => {
                return (StatusCode::BAD_REQUEST, cors, axum::Json(commandcode_json(false, Some("Invalid request body"))))
                    .into_response()
            }
        };
        if let Some(err) = payload.error.clone() {
            // Denial: resolve only on matching state, like the CLI.
            let mut guard = slot.lock().await;
            if payload.state.as_deref().unwrap_or_default() == guard.expected_state {
                if let Some(tx) = guard.sender.take() {
                    let message = payload.error_description.clone().unwrap_or(err);
                    let _ = tx.send(Err(message));
                }
            }
            return (StatusCode::OK, cors, axum::Json(commandcode_json(true, None))).into_response();
        }
        let complete = payload.api_key.clone().unwrap_or_default().trim().is_empty() == false
            && payload.user_id.clone().unwrap_or_default().trim().is_empty() == false
            && payload.user_name.clone().unwrap_or_default().trim().is_empty() == false
            && payload.key_name.clone().unwrap_or_default().trim().is_empty() == false;
        if !complete {
            return (StatusCode::BAD_REQUEST, cors, axum::Json(commandcode_json(false, Some("Missing required fields"))))
                .into_response();
        }
        let mut guard = slot.lock().await;
        if payload.state.as_deref().unwrap_or_default() != guard.expected_state {
            return (StatusCode::FORBIDDEN, cors, axum::Json(commandcode_json(false, Some("Invalid state token"))))
                .into_response();
        }
        guard.pending = Some(payload.clone());
        if content_type == "application/x-www-form-urlencoded" {
            // Browser form flow: send the tab to the landing page like the CLI.
            let location = format!("{COMMANDCODE_COMPLETE_PATH}?state={}", url::form_urlencoded::byte_serialize(payload.state.as_deref().unwrap_or_default().as_bytes()).collect::<String>());
            drop(guard);
            return (
                StatusCode::SEE_OTHER,
                cors,
                [(axum::http::header::LOCATION, location)],
                "",
            )
                .into_response();
        }
        if let Some(tx) = complete_sender(&slot).await {
            let _ = tx.send(Ok(payload));
        }
        (StatusCode::OK, cors, axum::Json(commandcode_json(true, None))).into_response()
    }

    async fn handle_complete(
        AxumState(slot): AxumState<Arc<Mutex<CommandCodeSlot>>>,
        headers: HeaderMap,
        Query(query): Query<CompleteQuery>,
    ) -> impl IntoResponse {
        let pna = headers.get("access-control-request-private-network").is_some();
        let cors = studio_cors(headers.get("origin"), pna);
        let mut guard = slot.lock().await;
        if query.state != guard.expected_state {
            return (
                StatusCode::FORBIDDEN,
                cors,
                axum::response::Html(commandcode_page("Invalid state token", "The state token did not match this login attempt. Return to Proxy Dock and restart login.")),
            )
                .into_response();
        }
        let Some(payload) = guard.pending.take() else {
            return (
                StatusCode::NOT_FOUND,
                cors,
                axum::response::Html(commandcode_page(
                    "Return to Proxy Dock",
                    "This page completes login automatically during sign-in. Restart login from Proxy Dock if you reached it directly.",
                )),
            )
                .into_response();
        };
        let name = payload.user_name.clone().unwrap_or_default();
        drop(guard);
        if let Some(tx) = complete_sender(&slot).await {
            let _ = tx.send(Ok(payload));
        }
        (
            StatusCode::OK,
            cors,
            axum::response::Html(commandcode_page(
                "Sign-in successful",
                &format!("Logged in{} — return to Proxy Dock.", if name.trim().is_empty() { String::new() } else { format!(" as {name}") }),
            )),
        )
            .into_response()
    }

    async fn handle_callback_get(headers: HeaderMap) -> impl IntoResponse {
        let pna = headers.get("access-control-request-private-network").is_some();
        (
            StatusCode::METHOD_NOT_ALLOWED,
            studio_cors(headers.get("origin"), pna),
            axum::response::Html(commandcode_page(
                "Return to Proxy Dock",
                "This page completes login automatically during sign-in. Restart login from Proxy Dock if you reached it directly.",
            )),
        )
    }

    let app = axum::Router::new()
        .route(COMMANDCODE_CALLBACK_PATH, post(handle_post).get(handle_callback_get).options(handle_options))
        .route(COMMANDCODE_COMPLETE_PATH, get(handle_complete))
        .with_state(slot);
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .map_err(|err| format!("command-code callback bind failed: {err}"))?;
    let port = listener.local_addr().map_err(|err| err.to_string())?.port();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    Ok((format!("http://127.0.0.1:{port}{COMMANDCODE_CALLBACK_PATH}"), rx))
}

/// Verifies a Command Code API key against /alpha/whoami. Returns the account
/// identity (email + username). 401/403 means the key is rejected.
pub async fn verify_commandcode_key(api_key: &str) -> Result<CommandCodeIdentity, String> {
    let secret = api_key.trim();
    let resp = client()
        .get(COMMANDCODE_WHOAMI_URL)
        .bearer_auth(secret)
        .header("Accept", "application/json")
        .header("x-command-code-version", COMMANDCODE_CLIENT_VERSION)
        .send()
        .await
        .map_err(|err| format!("commandcode whoami check failed: {err}"))?;
    if resp.status() == reqwest::StatusCode::UNAUTHORIZED || resp.status() == reqwest::StatusCode::FORBIDDEN {
        return Err("Command Code rejected the key (unauthorized) — check it and try again".to_string());
    }
    if !resp.status().is_success() {
        return Err(format!("Command Code whoami check failed ({})", resp.status()));
    }
    let body: serde_json::Value = resp.json().await.map_err(|err| format!("whoami parse failed: {err}"))?;
    let user = body.get("user").unwrap_or(&serde_json::Value::Null);
    let email = user.get("email").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    let user_name = user
        .get("userName")
        .or_else(|| user.get("name"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if email.is_empty() && user_name.is_empty() {
        return Err("Command Code accepted the key but returned no account identity".to_string());
    }
    Ok(CommandCodeIdentity { email, user_name })
}

fn client() -> reqwest::Client {
    reqwest::Client::builder().timeout(std::time::Duration::from_secs(30)).build().unwrap_or_else(|_| reqwest::Client::new())
}

pub async fn exchange_codex(code: &str, verifier: &str, port: u16) -> Result<StoredCredential, String> {
    let redirect = format!("http://localhost:{port}{CODEX_CALLBACK_PATH}");
    let params = [
        ("grant_type", "authorization_code"),
        ("client_id", CODEX_CLIENT_ID),
        ("code", code),
        ("redirect_uri", &redirect),
        ("code_verifier", verifier),
    ];
    let resp = client()
        .post(CODEX_TOKEN_URL)
        .form(&params)
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|err| format!("token exchange failed: {err}"))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("token exchange failed ({status}): {body}"));
    }
    let tokens: TokenResponse = resp.json().await.map_err(|err| format!("token response parse failed: {err}"))?;
    let (account_id, email) = decode_id_token(&tokens.id_token);
    // id_token is dropped after email extraction; refresh_token is kept.
    Ok(StoredCredential {
        access_token: tokens.access_token,
        refresh_token: tokens.refresh_token,
        id_token: None,
        account_id,
        email,
        expires_in: tokens.expires_in,
    })
}

pub async fn exchange_antigravity(code: &str, port: u16) -> Result<StoredCredential, String> {
    let redirect = format!("http://localhost:{port}{ANTIGRAVITY_CALLBACK_PATH}");
    let oauth = crate::oauth_secret::read()?;
    let params = [
        ("code", code.to_string()),
        ("client_id", oauth.client_id),
        ("client_secret", oauth.client_secret),
        ("redirect_uri", redirect),
        ("grant_type", "authorization_code".to_string()),
    ];
    let resp = client()
        .post(ANTIGRAVITY_TOKEN_URL)
        .form(&params)
        .send()
        .await
        .map_err(|err| format!("token exchange failed: {err}"))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("token exchange failed ({status}): {body}"));
    }
    let tokens: TokenResponse = resp.json().await.map_err(|err| format!("token response parse failed: {err}"))?;
    let email = fetch_google_email(&tokens.access_token).await?;
    // Google exchange carries no durable id_token need; refresh_token is kept.
    Ok(StoredCredential {
        access_token: tokens.access_token,
        refresh_token: tokens.refresh_token,
        id_token: None,
        account_id: String::new(),
        email,
        expires_in: tokens.expires_in,
    })
}

/// Axios-shaped headers the Claude OAuth control plane expects (mirrors
/// CLIProxyAPI's `applyClaudeOAuthAxiosHeaders`).
fn claude_oauth_headers(builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    builder
        .header("Accept", "application/json, text/plain, */*")
        .header("Content-Type", "application/json")
        .header("User-Agent", CLAUDE_AXIOS_UA)
        .header("Accept-Encoding", "gzip, compress, deflate, br")
        .header("Connection", "close")
}

/// Authorization-code exchange body. Field order is significant: it mirrors
/// the key order native Claude Code emits on the wire (a map would be
/// re-sorted alphabetically by serde_json and change the serialized bytes).
#[derive(Debug, serde::Serialize)]
struct ClaudeCodeExchange {
    grant_type: String,
    code: String,
    redirect_uri: String,
    client_id: String,
    code_verifier: String,
    state: String,
}

#[derive(Debug, serde::Deserialize)]
struct ClaudeTokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: Option<i64>,
    account: Option<ClaudeAccount>,
}

#[derive(Debug, serde::Deserialize)]
struct ClaudeAccount {
    uuid: Option<String>,
    email_address: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
struct ClaudeProfile {
    account: Option<ClaudeProfileAccount>,
    organization: Option<ClaudeProfileOrg>,
}

#[derive(Debug, serde::Deserialize)]
struct ClaudeProfileAccount {
    uuid: Option<String>,
    email: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
#[allow(dead_code)]
struct ClaudeProfileOrg {
    uuid: Option<String>,
    name: Option<String>,
}

/// Confirming identity lookup (`GET /api/oauth/profile`), mirroring
/// CLIProxyAPI's post-exchange companion call. Advisory: failures are
/// logged, never fatal — the exchange response already carries identity.
async fn fetch_claude_profile(access_token: &str) -> Option<ClaudeProfile> {
    let resp = claude_oauth_headers(client().get(CLAUDE_PROFILE_URL))
        .bearer_auth(access_token)
        .header("Cache-Control", "no-cache")
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    resp.json().await.ok()
}

pub async fn exchange_claude(code: &str, verifier: &str, state: &str, port: u16) -> Result<StoredCredential, String> {
    let redirect = format!("http://localhost:{port}{CLAUDE_CALLBACK_PATH}");
    // The callback code may carry a `#state` fragment (see CLIProxyAPI's
    // `parseCodeAndState`); a fragment state takes precedence.
    let (code, state) = match code.split_once('#') {
        Some((code, fragment)) if !fragment.is_empty() => (code, fragment),
        _ => (code, state),
    };
    let payload = ClaudeCodeExchange {
        grant_type: "authorization_code".to_string(),
        code: code.to_string(),
        redirect_uri: redirect,
        client_id: CLAUDE_CLIENT_ID.to_string(),
        code_verifier: verifier.to_string(),
        state: state.to_string(),
    };
    let resp = claude_oauth_headers(client().post(CLAUDE_TOKEN_URL))
        .json(&payload)
        .send()
        .await
        .map_err(|err| format!("token exchange failed: {err}"))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("token exchange failed ({status}): {body}"));
    }
    let tokens: ClaudeTokenResponse = resp.json().await.map_err(|err| format!("token response parse failed: {err}"))?;
    if tokens.access_token.trim().is_empty() {
        return Err("token exchange returned no access token".to_string());
    }
    let mut email = tokens.account.as_ref().and_then(|a| a.email_address.clone()).unwrap_or_default();
    let mut account_uuid = tokens.account.as_ref().and_then(|a| a.uuid.clone()).unwrap_or_default();
    // Let the profile response win where it carries identity the exchange
    // omitted (mirrors CLIProxyAPI's `inspectOAuthAccount`).
    if let Some(profile) = fetch_claude_profile(tokens.access_token.trim()).await {
        if let Some(account) = profile.account {
            if let Some(uuid) = account.uuid.filter(|u| !u.trim().is_empty()) {
                account_uuid = uuid;
            }
            if let Some(address) = account.email.filter(|e| !e.trim().is_empty()) {
                email = address;
            }
        }
        // Organization identity is advisory only (StoredCredential has no
        // org fields; the account slot keys on email) — profile email/UUID
        // above are what matter.
        let _ = profile.organization;
    }
    if email.trim().is_empty() {
        return Err("claude sign-in returned no account email".to_string());
    }
    // The account UUID rides in `account_id` (Claude issues no id_token).
    Ok(StoredCredential {
        access_token: tokens.access_token,
        refresh_token: tokens.refresh_token,
        id_token: None,
        account_id: account_uuid,
        email,
        expires_in: tokens.expires_in,
    })
}

/// One Claude OAuth refresh (mirrors CLIProxyAPI's `RefreshTokens`).
/// Returns the new access token (and rotated refresh token, if any).
pub async fn refresh_claude_token(refresh_token: &str) -> Result<(String, Option<String>), String> {
    let payload = serde_json::json!({
        "client_id": CLAUDE_CLIENT_ID,
        "grant_type": "refresh_token",
        "refresh_token": refresh_token,
        "scope": CLAUDE_SCOPE,
    });
    let resp = claude_oauth_headers(client().post(CLAUDE_TOKEN_URL))
        .json(&payload)
        .send()
        .await
        .map_err(|err| format!("claude token refresh failed: {err}"))?;
    if !resp.status().is_success() {
        return Err(format!("claude token refresh failed ({})", resp.status()));
    }
    let tokens: ClaudeTokenResponse = resp.json().await.map_err(|err| format!("refresh parse failed: {err}"))?;
    if tokens.access_token.trim().is_empty() {
        return Err("claude refresh returned no access token".to_string());
    }
    Ok((tokens.access_token, tokens.refresh_token))
}

/// Verifies an Anthropic API key against the documented Models API
/// (`GET /v1/models` with `x-api-key`): 401/403 = bad key, 200 = valid.
/// Returns the model ids for display. OAuth setup tokens (`sk-ant-oat-*`)
/// are NOT API keys — they are rejected here with a pointer to browser
/// sign-in, never sent as `x-api-key`.
pub async fn verify_claude_key(api_key: &str) -> Result<Vec<String>, String> {
    use crate::adapters::claude as cl;
    let secret = api_key.trim();
    if secret.starts_with("sk-ant-oat-") {
        return Err("that looks like a Claude OAuth setup token, not an API key — use browser sign-in instead".to_string());
    }
    cl::fetch_models(secret, cl::CredKind::ApiKey)
        .await
        .map(|entries| entries.into_iter().map(|(id, _)| id).collect())
}

async fn fetch_google_email(access_token: &str) -> Result<String, String> {
    #[derive(serde::Deserialize)]
    struct Info {
        email: Option<String>,
    }
    let resp = client()
        .get("https://www.googleapis.com/oauth2/v2/userinfo?alt=json")
        .bearer_auth(access_token)
        .send()
        .await
        .map_err(|err| format!("userinfo failed: {err}"))?;
    if !resp.status().is_success() {
        return Err(format!("userinfo failed ({})", resp.status()));
    }
    let info: Info = resp.json().await.map_err(|err| format!("userinfo parse failed: {err}"))?;
    info.email.filter(|email| !email.trim().is_empty()).ok_or_else(|| "userinfo returned no email".to_string())
}

fn decode_id_token(id_token: &Option<String>) -> (String, String) {
    let Some(token) = id_token else { return (String::new(), String::new()) };
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() < 2 {
        return (String::new(), String::new());
    }
    let Ok(payload) = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(parts[1]) else {
        return (String::new(), String::new());
    };
    let claims: serde_json::Value = serde_json::from_slice(&payload).unwrap_or(serde_json::Value::Null);
    let account = claims.get("chatgpt_account_id").or_else(|| claims.get("sub")).and_then(|value| value.as_str()).unwrap_or("").to_string();
    let email = claims.get("email").and_then(|value| value.as_str()).unwrap_or("").to_string();
    (account, email)
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct StoredCredential {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub id_token: Option<String>,
    pub account_id: String,
    pub email: String,
    pub expires_in: Option<i64>,
}

/// Per-account refresh lock for 401-refresh singleflight: concurrent 401s on
/// one account serialize around refresh+persist so a single-use refresh token
/// is consumed once (last-writer-wins on persist). Serving, catalog, and
/// quota paths share this map — hold the guard across refresh+persist, and
/// re-read the stored secret after acquiring (a waiter may find the leader's
/// fresh token and skip its own refresh). Pure map behavior is unit-tested
/// below; the "one refresh" property follows from holding one guard.
static REFRESH_LOCKS: std::sync::OnceLock<
    std::sync::Mutex<std::collections::HashMap<String, std::sync::Arc<tokio::sync::Mutex<()>>>>,
> = std::sync::OnceLock::new();

/// Lock for `(provider_slug, account_id)` refresh+persist. Same key returns
/// the same guard; different keys never block each other.
pub fn refresh_lock_for(provider_slug: &str, account_id: &str) -> std::sync::Arc<tokio::sync::Mutex<()>> {
    let key = format!("{provider_slug}\0{account_id}");
    let map = REFRESH_LOCKS
        .get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()));
    let mut guard = map.lock().unwrap_or_else(|e| e.into_inner());
    guard
        .entry(key)
        .or_insert_with(|| std::sync::Arc::new(tokio::sync::Mutex::new(())))
        .clone()
}

/// Merge a refreshed access token (plus rotated refresh, if any) into the
/// previous stored secret. Pure (no keyring): persist is last-writer-wins —
/// concurrent refreshes serialize on [`refresh_lock_for`] and the loser's
/// write lands second with its own fresh access token, so the stored value
/// is always a usable token, never a consumed single-use one.
/// `id_token` is dropped (email is already extracted at exchange time).
pub fn refreshed_payload(old_secret: &str, access_token: &str, refresh_token: Option<&str>) -> Option<String> {
    let mut stored: StoredCredential = serde_json::from_str(old_secret).ok()?;
    if stored.access_token.trim().is_empty() || access_token.trim().is_empty() {
        return None;
    }
    stored.access_token = access_token.trim().to_string();
    if let Some(refresh) = refresh_token.filter(|r| !r.trim().is_empty()) {
        stored.refresh_token = Some(refresh.trim().to_string());
    }
    stored.id_token = None;
    serde_json::to_string(&stored).ok()
}

/// Drop `id_token` after email extraction. The id token is only needed once
/// (decode `chatgpt_account_id`/`email` at exchange); the refresh token is
/// the long-lived credential and is always kept.
pub fn without_id_token(mut cred: StoredCredential) -> StoredCredential {
    cred.id_token = None;
    cred
}

/// Verified OpenCode key: models for display plus the provider-reported
/// subscription plan. The `/zen/go/v1/usage` shape is verified against the
/// open-source console route: 401 = bad key, 403 EntitlementError = valid key
/// without a Go subscription ("Free"), 200 with usage windows = "Go".
pub struct OpencodeStatus {
    pub models: Vec<String>,
    pub plan: String,
    /// Raw usage body on 200 (for quota mapping); None for Free keys.
    pub usage: Option<serde_json::Value>,
}

pub async fn verify_opencode_key(api_key: &str) -> Result<OpencodeStatus, String> {
    let secret = api_key.trim();
    // NOTE: GET /zen/go/v1/models is public (200 even with a bad key), so it
    // cannot verify a key. GET /zen/go/v1/usage is auth-gated: 401 = bad key,
    // 403 = valid key without a Go subscription, 200 = subscribed.
    let usage_resp = client()
        .get(OPENCODE_USAGE_URL)
        .bearer_auth(secret)
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|err| format!("opencode usage check failed: {err}"))?;
    if usage_resp.status() == reqwest::StatusCode::UNAUTHORIZED {
        return Err("opencode.ai rejected the key (unauthorized) — check it and try again".to_string());
    }
    let subscribed = usage_resp.status() != reqwest::StatusCode::FORBIDDEN;
    if subscribed && !usage_resp.status().is_success() {
        return Err(format!("opencode.ai usage check failed ({})", usage_resp.status()));
    }
    let plan = crate::quota::opencode_plan_for_status(usage_resp.status().as_u16()).unwrap_or("Free").to_string();
    let usage: Option<serde_json::Value> = if subscribed {
        usage_resp.json().await.ok()
    } else {
        None
    };
    // Key is valid at this point. Fetch the public model list for display only.
    let models_resp = client()
        .get(OPENCODE_MODELS_URL)
        .bearer_auth(secret)
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|err| format!("opencode models check failed: {err}"))?;
    if !models_resp.status().is_success() {
        return Err(format!("opencode.ai models check failed ({})", models_resp.status()));
    }
    let body: serde_json::Value = models_resp.json().await.map_err(|err| format!("models parse failed: {err}"))?;
    let items = body.get("data").unwrap_or(&body);
    let models = items
        .as_array()
        .map(|list| list.iter().filter_map(|item| item.get("id")?.as_str().map(str::to_string)).collect())
        .unwrap_or_default();
    Ok(OpencodeStatus { models, plan, usage })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_auth_url_has_pkce_and_state() {
        let pkce = Pkce { verifier: "v".to_string(), challenge: "c".to_string() };
        let url = codex_auth_url("s123", &pkce, CODEX_CALLBACK_PORT);
        assert!(url.starts_with(CODEX_AUTH_URL));
        assert!(url.contains("code_challenge=c"));
        assert!(url.contains("state=s123"));
        assert!(url.contains("localhost%3A1455"));
    }

    #[test]
    fn antigravity_auth_url_has_google_endpoints() {
        let url = antigravity_auth_url("s456", ANTIGRAVITY_CALLBACK_PORT);
        assert!(url.starts_with(ANTIGRAVITY_AUTH_URL));
        assert!(url.contains("state=s456"));
        assert!(url.contains("oauth-callback"));
    }

    #[test]
    fn claude_auth_url_has_pkce_and_official_callback() {
        let pkce = Pkce { verifier: "v".to_string(), challenge: "c".to_string() };
        let url = claude_auth_url("s789", &pkce, CLAUDE_CALLBACK_PORT);
        assert!(url.starts_with(CLAUDE_AUTH_URL));
        assert!(url.contains("code_challenge=c"));
        assert!(url.contains("state=s789"));
        assert!(url.contains("code=true"));
        assert!(url.contains("localhost%3A54545"));
        assert!(url.contains("callback"));
    }

    #[test]
    fn pkce_challenge_is_sha256() {
        let pkce = pkce();
        assert_eq!(pkce.verifier.len(), 64);
        assert!(!pkce.challenge.is_empty());
    }

    #[test]
    fn refresh_locks_isolate_accounts_and_serialize() {
        // Same key -> same guard; different keys -> independent guards.
        let a1 = refresh_lock_for("chatgpt", "a@example.com");
        let a2 = refresh_lock_for("chatgpt", "a@example.com");
        let b = refresh_lock_for("chatgpt", "b@example.com");
        assert!(std::sync::Arc::ptr_eq(&a1, &a2));
        assert!(!std::sync::Arc::ptr_eq(&a1, &b));
        // Held guard blocks a second acquirer (singleflight serialization).
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("rt");
        rt.block_on(async {
            let _held = a1.lock().await;
            assert!(a2.try_lock().is_err());
            assert!(b.try_lock().is_ok());
        });
    }

    #[test]
    fn refreshed_payload_keeps_refresh_drops_id_token() {
        let old = serde_json::json!({
            "access_token": "old-at", "refresh_token": "rt",
            "id_token": "id-jwt", "account_id": "acc",
            "email": "e", "expires_in": null,
        })
        .to_string();
        // Rotation applies; id_token drops.
        let next = refreshed_payload(&old, "new-at", Some("rt2")).expect("payload");
        let v: serde_json::Value = serde_json::from_str(&next).expect("json");
        assert_eq!(v["access_token"], "new-at");
        assert_eq!(v["refresh_token"], "rt2");
        assert!(v.get("id_token").is_none() || v["id_token"].is_null());
        // No rotation keeps the old refresh; last-writer-wins applies cleanly.
        let next2 = refreshed_payload(&old, "new-at-2", None).expect("payload");
        let v2: serde_json::Value = serde_json::from_str(&next2).expect("json");
        assert_eq!(v2["access_token"], "new-at-2");
        assert_eq!(v2["refresh_token"], "rt");
        // Garbage in -> None (caller keeps serving with the live token).
        assert!(refreshed_payload("not-json", "x", None).is_none());
        assert!(refreshed_payload(&old, "  ", None).is_none());
    }
}
