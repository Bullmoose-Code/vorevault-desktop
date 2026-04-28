use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use rand::RngCore;
use sha2::{Digest, Sha256};

/// Default vault URL when `VAULT_URL` env var isn't set.
pub const DEFAULT_VAULT_URL: &str = "https://vault.bullmoosefn.com";

/// Read the vault URL from env, falling back to the default. Trailing slashes
/// are stripped so callers can do `format!("{vault}/api/...")` safely.
pub fn vault_url_from_env() -> String {
    let raw = std::env::var("VAULT_URL").unwrap_or_else(|_| DEFAULT_VAULT_URL.to_string());
    raw.trim_end_matches('/').to_string()
}

/// Generate a PKCE `code_verifier` per RFC 7636 §4.1: 32 random bytes
/// base64url-encoded with no padding (43 ASCII chars from the unreserved set).
/// Kept secret on the desktop side — never sent in any URL.
pub fn generate_code_verifier() -> String {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

/// PKCE S256 transform per RFC 7636 §4.2: base64url(SHA256(code_verifier))
/// with no padding. Output is exactly 43 chars.
pub fn compute_code_challenge(code_verifier: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(code_verifier.as_bytes());
    URL_SAFE_NO_PAD.encode(hasher.finalize())
}

/// Build the URL the desktop opens in the system browser to start the OAuth
/// flow. The vault server then sets a state cookie and redirects to Discord.
pub fn build_init_url(vault_url: &str, port: u16, code_challenge: &str) -> String {
    format!(
        "{}/api/auth/desktop-init?port={}&code_challenge={}",
        vault_url.trim_end_matches('/'),
        port,
        urlencoding::encode(code_challenge),
    )
}

use serde::{Deserialize, Serialize};
use std::time::Duration;
use tiny_http::{Method, Response, Server};

/// Bundled at compile time so we have a self-contained binary.
const SUCCESS_HTML: &str = include_str!("../../ui-callback/success.html");

/// Errors that can occur during sign in. All variants imply the keychain is
/// not modified and the user is still effectively signed out.
#[derive(Debug)]
pub enum AuthError {
    BindFailed(String),
    BrowserOpenFailed(String),
    Timeout,
    BadCallback(String),
    ExchangeFailed(String),
    KeychainFailed(keyring::Error),
}

impl std::fmt::Display for AuthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AuthError::BindFailed(s) => write!(f, "couldn't bind localhost listener: {}", s),
            AuthError::BrowserOpenFailed(s) => write!(f, "couldn't open browser: {}", s),
            AuthError::Timeout => write!(f, "sign in timed out"),
            AuthError::BadCallback(s) => write!(f, "bad OAuth callback: {}", s),
            AuthError::ExchangeFailed(s) => write!(f, "couldn't exchange auth code: {}", s),
            AuthError::KeychainFailed(e) => write!(f, "couldn't save credentials: {}", e),
        }
    }
}

impl std::error::Error for AuthError {}

/// Time to wait for the user to complete the OAuth flow in their browser.
const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(300);

/// Body sent to the vault's POST /api/auth/desktop-exchange endpoint.
#[derive(Debug, Serialize)]
struct ExchangeRequest<'a> {
    code: &'a str,
    code_verifier: &'a str,
}

/// Body returned from the exchange endpoint on success.
#[derive(Debug, Deserialize)]
struct ExchangeResponse {
    session_token: String,
}

/// Run the PKCE-style sign-in flow:
/// 1. Generate code_verifier (kept secret) + code_challenge (sent in browser URL).
/// 2. Bind a localhost listener on a free port.
/// 3. Open the system browser to the vault's `desktop-init` route with the challenge.
/// 4. Block until the browser is redirected to our listener with `?code=<auth_code>`.
/// 5. POST {code, code_verifier} to `/api/auth/desktop-exchange` to redeem the session token.
/// 6. Store the session token in the OS keychain.
///
/// On any failure, the keychain is not modified.
///
/// `open_browser` is injected so this function is testable without actually
/// launching a browser.
pub fn sign_in<F>(vault_url: &str, open_browser: F) -> Result<String, AuthError>
where
    F: FnOnce(&str) -> Result<(), String>,
{
    let server = Server::http("127.0.0.1:0").map_err(|e| AuthError::BindFailed(e.to_string()))?;
    let port = server
        .server_addr()
        .to_ip()
        .map(|s| s.port())
        .ok_or_else(|| AuthError::BindFailed("couldn't read bound port".into()))?;

    let code_verifier = generate_code_verifier();
    let code_challenge = compute_code_challenge(&code_verifier);
    let url = build_init_url(vault_url, port, &code_challenge);

    open_browser(&url).map_err(AuthError::BrowserOpenFailed)?;

    // Block on a single request, then close the listener.
    let auth_code = match server.recv_timeout(SIGN_IN_TIMEOUT) {
        Ok(Some(req)) => extract_code_from_request(req)?,
        Ok(None) => return Err(AuthError::Timeout),
        Err(e) => return Err(AuthError::BadCallback(format!("listener error: {}", e))),
    };

    drop(server); // close the listener

    // Exchange the code for the session token via HTTPS POST.
    let session_token = exchange_code(vault_url, &auth_code, &code_verifier)?;

    crate::keychain::store(&session_token).map_err(AuthError::KeychainFailed)?;
    Ok(session_token)
}

/// Pull `?code=<auth_code>` from the callback request, respond with success
/// HTML, and return the auth code. Validates that the request is GET / with a
/// non-empty `code` query param.
fn extract_code_from_request(req: tiny_http::Request) -> Result<String, AuthError> {
    if *req.method() != Method::Get {
        let _ = req.respond(Response::from_string("Method not allowed").with_status_code(405));
        return Err(AuthError::BadCallback("not a GET".to_string()));
    }

    let full = format!("http://127.0.0.1{}", req.url());
    let parsed = match url::Url::parse(&full) {
        Ok(u) => u,
        Err(e) => {
            let _ = req.respond(Response::from_string("Bad request").with_status_code(400));
            return Err(AuthError::BadCallback(format!(
                "malformed callback url: {}",
                e
            )));
        }
    };

    let code = parsed
        .query_pairs()
        .find(|(k, _)| k == "code")
        .map(|(_, v)| v.into_owned());
    let Some(code) = code else {
        let _ = req.respond(Response::from_string("Missing code param").with_status_code(400));
        return Err(AuthError::BadCallback("missing code param".to_string()));
    };

    if code.is_empty() {
        let _ = req.respond(Response::from_string("Empty code").with_status_code(400));
        return Err(AuthError::BadCallback("empty code".to_string()));
    }

    let response = Response::from_string(SUCCESS_HTML)
        .with_status_code(200)
        .with_header(
            "Content-Type: text/html; charset=utf-8"
                .parse::<tiny_http::Header>()
                .unwrap(),
        );
    let _ = req.respond(response);

    Ok(code)
}

/// POST {code, code_verifier} to /api/auth/desktop-exchange. Returns the
/// session_token from the response body on success, or AuthError::ExchangeFailed
/// on any non-success status / network failure / parse failure.
fn exchange_code(vault_url: &str, code: &str, code_verifier: &str) -> Result<String, AuthError> {
    let url = format!(
        "{}/api/auth/desktop-exchange",
        vault_url.trim_end_matches('/')
    );
    let body = ExchangeRequest {
        code,
        code_verifier,
    };
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| AuthError::ExchangeFailed(format!("client build: {}", e)))?;

    let resp = client
        .post(&url)
        .json(&body)
        .send()
        .map_err(|e| AuthError::ExchangeFailed(format!("request: {}", e)))?;

    let status = resp.status();
    if !status.is_success() {
        return Err(AuthError::ExchangeFailed(format!(
            "vault returned {}",
            status.as_u16()
        )));
    }

    let parsed: ExchangeResponse = resp
        .json()
        .map_err(|e| AuthError::ExchangeFailed(format!("parse response: {}", e)))?;
    Ok(parsed.session_token)
}

/// Snapshot of the desktop's auth state, derived from keychain + a server check.
#[derive(Debug, Clone)]
pub struct AuthState {
    pub username: Option<String>,
}

#[derive(Debug, Deserialize)]
struct MeResponse {
    user: Option<MeUser>,
}

#[derive(Debug, Deserialize)]
struct MeUser {
    #[allow(dead_code)]
    id: String,
    username: String,
    #[allow(dead_code)]
    is_admin: bool,
}

/// Resolve the current auth state by checking the keychain, then asking the
/// server whether the stored session is still valid.
///
/// - No keychain entry → `{username: None}`
/// - Keychain entry, server returns 200 → `{username: Some(...)}`
/// - Keychain entry, server returns 401 → delete keychain, `{username: None}`
/// - Keychain entry, network/other error → preserve keychain, `{username: None}`
///   (transient state; we'll recheck on the next launch)
pub fn current_state(vault_url: &str) -> AuthState {
    let token = match crate::keychain::load() {
        Ok(Some(t)) => t,
        Ok(None) => return AuthState { username: None },
        Err(e) => {
            log::warn!("keychain load failed: {}", e);
            return AuthState { username: None };
        }
    };

    let url = format!("{}/api/auth/me", vault_url.trim_end_matches('/'));
    let cookie = format!("vv_session={}", token);
    let client = match reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            log::warn!("reqwest client build failed: {}", e);
            return AuthState { username: None };
        }
    };

    let resp = match client.get(&url).header("Cookie", cookie).send() {
        Ok(r) => r,
        Err(e) => {
            log::warn!("/api/auth/me request failed: {}", e);
            return AuthState { username: None };
        }
    };

    if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
        let _ = crate::keychain::delete();
        return AuthState { username: None };
    }

    if !resp.status().is_success() {
        log::warn!("/api/auth/me returned {}", resp.status());
        return AuthState { username: None };
    }

    match resp.json::<MeResponse>() {
        Ok(MeResponse { user: Some(u) }) => AuthState {
            username: Some(u.username),
        },
        Ok(MeResponse { user: None }) => AuthState { username: None },
        Err(e) => {
            log::warn!("failed to parse /api/auth/me response: {}", e);
            AuthState { username: None }
        }
    }
}

/// Sign out: best-effort POST to /api/auth/logout, then delete the keychain
/// entry regardless of whether the server call succeeded. Local sign-out
/// always works — the worst case is a stale session row that the server's
/// 30-day expiry will eventually GC.
pub fn sign_out(vault_url: &str) {
    if let Ok(Some(token)) = crate::keychain::load() {
        let url = format!("{}/api/auth/logout", vault_url.trim_end_matches('/'));
        let cookie = format!("vv_session={}", token);
        if let Ok(client) = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
        {
            let _ = client.post(&url).header("Cookie", cookie).send();
        }
    }
    let _ = crate::keychain::delete();
}

#[cfg(test)]
mod tests {
    use super::*;

    // Single test for both VAULT_URL behaviors. cargo runs tests in parallel
    // by default, so two separate tests both mutating the same env var would
    // race. Sequential ownership inside one test eliminates the race.
    #[test]
    fn vault_url_from_env_resolution() {
        std::env::set_var("VAULT_URL", "https://example.com/");
        assert_eq!(vault_url_from_env(), "https://example.com");
        std::env::remove_var("VAULT_URL");
        assert_eq!(vault_url_from_env(), DEFAULT_VAULT_URL);
    }

    #[test]
    fn code_verifier_is_43_char_base64url() {
        let v = generate_code_verifier();
        assert_eq!(v.len(), 43);
        assert!(v
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'));
    }

    #[test]
    fn code_challenge_is_43_char_base64url() {
        let c = compute_code_challenge("any-verifier");
        assert_eq!(c.len(), 43);
        assert!(c
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'));
    }

    #[test]
    fn code_challenge_matches_rfc_7636_example_vector() {
        // Per RFC 7636 §4.2:
        // code_verifier  = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"
        // code_challenge = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        let v = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        assert_eq!(
            compute_code_challenge(v),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn build_init_url_includes_port_and_challenge() {
        let url = build_init_url("https://vault.example.com", 42876, "abc123");
        assert_eq!(
            url,
            "https://vault.example.com/api/auth/desktop-init?port=42876&code_challenge=abc123"
        );
    }

    #[test]
    fn build_init_url_strips_trailing_slash_from_vault_url() {
        let url = build_init_url("https://vault.example.com/", 42876, "abc");
        assert_eq!(
            url,
            "https://vault.example.com/api/auth/desktop-init?port=42876&code_challenge=abc"
        );
    }
}
