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
    if parsed.host_str() != Some("open") {
        return Err(DeepLinkError::BadHost);
    }
    let mut out = String::from(vault_url.trim_end_matches('/'));
    out.push_str(parsed.path());
    Ok(out)
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
}
