//! `vorevault://` deep-link translation and dispatch.
//!
//! Translation is pure: takes a `vorevault://...` string and the configured
//! vault URL, returns an `https://<vault>/...` string. Dispatch is the thin
//! Tauri-aware wrapper that calls `tauri_plugin_opener::open_url` with the
//! translated target.

#[derive(Debug)]
pub enum DeepLinkError {
    Parse(url::ParseError),
    BadScheme,
    BadHost,
    HasCredentials,
    HasPort,
    BadPath,
}

impl std::fmt::Display for DeepLinkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DeepLinkError::Parse(e) => write!(f, "parse: {}", e),
            DeepLinkError::BadScheme => write!(f, "scheme must be 'vorevault'"),
            DeepLinkError::BadHost => write!(f, "host must be 'open'"),
            DeepLinkError::HasCredentials => write!(f, "URL must not contain user/password"),
            DeepLinkError::HasPort => write!(f, "URL must not contain a port"),
            DeepLinkError::BadPath => write!(f, "path must begin with '/'"),
        }
    }
}

impl std::error::Error for DeepLinkError {}

impl From<url::ParseError> for DeepLinkError {
    fn from(e: url::ParseError) -> Self {
        DeepLinkError::Parse(e)
    }
}

use url::Url;

/// Translate a `vorevault://...` URL into an `https://<vault>/...` URL.
/// The output's scheme + host come entirely from `vault_url`; only the path,
/// query, and fragment of the input pass through. There is no input that can
/// produce a non-vault target URL (security by construction).
pub fn translate(input: &str, vault_url: &str) -> Result<String, DeepLinkError> {
    let parsed = Url::parse(input)?;
    if parsed.scheme() != "vorevault" {
        return Err(DeepLinkError::BadScheme);
    }
    if parsed.host_str().map(str::to_ascii_lowercase).as_deref() != Some("open") {
        return Err(DeepLinkError::BadHost);
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(DeepLinkError::HasCredentials);
    }
    if parsed.port().is_some() {
        return Err(DeepLinkError::HasPort);
    }
    let path = parsed.path();
    if !path.starts_with('/') {
        return Err(DeepLinkError::BadPath);
    }
    let mut out = String::from(vault_url.trim_end_matches('/'));
    out.push_str(path);
    if let Some(q) = parsed.query() {
        out.push('?');
        out.push_str(q);
    }
    if let Some(f) = parsed.fragment() {
        out.push('#');
        out.push_str(f);
    }
    Ok(out)
}

/// Translate `raw_url` and hand the result to the system browser. All errors
/// (parse, validation, browser-open failure) are logged but never surfaced to
/// the user — the user clicked a link from elsewhere and a notification or
/// modal here would be confusing and out of context.
///
/// `_app` is taken (not used today) so future expansion (e.g. focusing the
/// settings window for certain link types) does not require changing every
/// call site.
pub fn dispatch(_app: &tauri::AppHandle, raw_url: &str) {
    let vault = crate::auth::vault_url_from_env();
    match translate(raw_url, &vault) {
        Ok(target) => {
            log::info!("deep link → {}", target);
            if let Err(e) = tauri_plugin_opener::open_url(&target, None::<&str>) {
                log::warn!("deep link: failed to open browser for {}: {}", target, e);
            }
        }
        Err(e) => {
            log::warn!("deep link: rejected input {:?}: {}", raw_url, e);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn translates_canonical_file_link() {
        let out = translate(
            "vorevault://open/files/abc-123",
            "https://vault.bullmoosefn.com",
        )
        .expect("happy path should succeed");
        assert_eq!(out, "https://vault.bullmoosefn.com/files/abc-123");
    }

    #[test]
    fn rejects_wrong_scheme() {
        let result = translate(
            "https://attacker.example.com/files/abc",
            "https://vault.bullmoosefn.com",
        );
        assert!(matches!(result, Err(DeepLinkError::BadScheme)));
    }

    #[test]
    fn rejects_wrong_host() {
        let result = translate(
            "vorevault://attacker.example.com/files/abc",
            "https://vault.bullmoosefn.com",
        );
        assert!(matches!(result, Err(DeepLinkError::BadHost)));
    }

    #[test]
    fn rejects_credentials() {
        let with_user = translate(
            "vorevault://user@open/files/abc",
            "https://vault.bullmoosefn.com",
        );
        assert!(matches!(with_user, Err(DeepLinkError::HasCredentials)));

        let with_password = translate(
            "vorevault://user:pw@open/files/abc",
            "https://vault.bullmoosefn.com",
        );
        assert!(matches!(with_password, Err(DeepLinkError::HasCredentials)));
    }

    #[test]
    fn rejects_port() {
        let result = translate(
            "vorevault://open:8080/files/abc",
            "https://vault.bullmoosefn.com",
        );
        assert!(matches!(result, Err(DeepLinkError::HasPort)));
    }

    #[test]
    fn rejects_missing_path() {
        // `vorevault://open` (no path) parses with an empty path. Reject so
        // callers must be explicit about what they want opened.
        let result = translate("vorevault://open", "https://vault.bullmoosefn.com");
        assert!(matches!(result, Err(DeepLinkError::BadPath)));
    }

    #[test]
    fn allows_bare_vault_root() {
        let out = translate("vorevault://open/", "https://vault.bullmoosefn.com")
            .expect("bare vault root should be allowed");
        assert_eq!(out, "https://vault.bullmoosefn.com/");
    }

    #[test]
    fn passes_query_string_through() {
        let out = translate(
            "vorevault://open/files/abc?tag=apex&page=2",
            "https://vault.bullmoosefn.com",
        )
        .expect("query passthrough should succeed");
        assert_eq!(
            out,
            "https://vault.bullmoosefn.com/files/abc?tag=apex&page=2"
        );
    }

    #[test]
    fn preserves_query_url_encoding() {
        // The `url` crate parses `?tag=foo%20bar` and gives back `query()` =
        // `"tag=foo%20bar"` (the encoded form, NOT decoded). Confirm we do not
        // accidentally re-encode.
        let out = translate(
            "vorevault://open/search?q=foo%20bar",
            "https://vault.bullmoosefn.com",
        )
        .expect("query passthrough should succeed");
        assert_eq!(out, "https://vault.bullmoosefn.com/search?q=foo%20bar");
    }

    #[test]
    fn passes_fragment_through() {
        let out = translate(
            "vorevault://open/files/abc#t=10s",
            "https://vault.bullmoosefn.com",
        )
        .expect("fragment passthrough should succeed");
        assert_eq!(out, "https://vault.bullmoosefn.com/files/abc#t=10s");
    }

    #[test]
    fn passes_query_and_fragment_together() {
        let out = translate(
            "vorevault://open/files/abc?autoplay=1#t=10",
            "https://vault.bullmoosefn.com",
        )
        .expect("query+fragment together should succeed");
        assert_eq!(
            out,
            "https://vault.bullmoosefn.com/files/abc?autoplay=1#t=10"
        );
    }

    #[test]
    fn vault_url_trailing_slash_is_trimmed() {
        let out = translate(
            "vorevault://open/files/abc",
            "https://vault.bullmoosefn.com/", // note trailing slash
        )
        .expect("trailing-slash vault URL should still produce a clean output");
        assert_eq!(out, "https://vault.bullmoosefn.com/files/abc");
    }

    #[test]
    fn vault_url_with_dev_port() {
        let out = translate("vorevault://open/files/abc", "http://localhost:3000")
            .expect("dev vault URL should work");
        assert_eq!(out, "http://localhost:3000/files/abc");
    }

    #[test]
    fn vault_url_with_dev_port_and_trailing_slash() {
        let out = translate("vorevault://open/files/abc", "http://localhost:3000/")
            .expect("dev vault URL with trailing slash should work");
        assert_eq!(out, "http://localhost:3000/files/abc");
    }

    #[test]
    fn vault_url_with_subpath_mount() {
        // Hypothetical: vault deployed at example.com/vv. Translator must not
        // break on a vault URL that already has a path component.
        let out = translate("vorevault://open/files/abc", "https://example.com/vv")
            .expect("sub-path mounted vault URL should work");
        assert_eq!(out, "https://example.com/vv/files/abc");
    }

    #[test]
    fn host_comparison_is_effectively_case_insensitive() {
        // The `url` crate normalizes hosts to lowercase during parse, so
        // by the time we compare against the literal `"open"`, an input of
        // `"OPEN"` has already become `"open"`. Confirm.
        let out = translate(
            "vorevault://OPEN/files/abc",
            "https://vault.bullmoosefn.com",
        )
        .expect("upper-case host should be normalized and accepted");
        assert_eq!(out, "https://vault.bullmoosefn.com/files/abc");
    }

    #[test]
    fn scheme_comparison_is_effectively_case_insensitive() {
        let out = translate(
            "VOREVAULT://open/files/abc",
            "https://vault.bullmoosefn.com",
        )
        .expect("upper-case scheme should be normalized and accepted");
        assert_eq!(out, "https://vault.bullmoosefn.com/files/abc");
    }

    #[test]
    fn rejects_unparseable_input() {
        let result = translate("not a url", "https://vault.bullmoosefn.com");
        assert!(matches!(result, Err(DeepLinkError::Parse(_))));
    }

    #[test]
    fn rejects_empty_input() {
        let result = translate("", "https://vault.bullmoosefn.com");
        assert!(matches!(result, Err(DeepLinkError::Parse(_))));
    }
}
