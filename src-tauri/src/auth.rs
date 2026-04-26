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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vault_url_strips_trailing_slash() {
        std::env::set_var("VAULT_URL", "https://example.com/");
        assert_eq!(vault_url_from_env(), "https://example.com");
        std::env::remove_var("VAULT_URL");
    }

    #[test]
    fn vault_url_uses_default_when_unset() {
        std::env::remove_var("VAULT_URL");
        assert_eq!(vault_url_from_env(), DEFAULT_VAULT_URL);
    }

    #[test]
    fn code_verifier_is_43_char_base64url() {
        let v = generate_code_verifier();
        assert_eq!(v.len(), 43);
        assert!(v.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'));
    }

    #[test]
    fn code_challenge_is_43_char_base64url() {
        let c = compute_code_challenge("any-verifier");
        assert_eq!(c.len(), 43);
        assert!(c.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'));
    }

    #[test]
    fn code_challenge_matches_rfc_7636_example_vector() {
        // Per RFC 7636 §4.2:
        // code_verifier  = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"
        // code_challenge = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        let v = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        assert_eq!(compute_code_challenge(v), "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
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
