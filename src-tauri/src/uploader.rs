//! tus protocol uploader. Sends Cookie-authenticated uploads to vault.

use base64::{engine::general_purpose::STANDARD, Engine as _};
use std::path::Path;
use std::time::Duration;

const TUS_RESUMABLE: &str = "1.0.0";
const PATCH_CHUNK_SIZE: usize = 5 * 1024 * 1024; // 5 MB

#[derive(Debug)]
pub enum UploadError {
    Io(std::io::Error),
    Reqwest(reqwest::Error),
    BadStatus(u16),
    /// The session is no longer valid — caller should pause the queue.
    Unauthorized,
    /// Server reports the file is too big.
    TooLarge,
    NoLocationHeader,
}

impl std::fmt::Display for UploadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UploadError::Io(e) => write!(f, "io: {}", e),
            UploadError::Reqwest(e) => write!(f, "http: {}", e),
            UploadError::BadStatus(s) => write!(f, "bad status: {}", s),
            UploadError::Unauthorized => write!(f, "session expired"),
            UploadError::TooLarge => write!(f, "file too large"),
            UploadError::NoLocationHeader => write!(f, "tus POST returned no Location header"),
        }
    }
}

impl std::error::Error for UploadError {}

impl From<std::io::Error> for UploadError {
    fn from(e: std::io::Error) -> Self {
        UploadError::Io(e)
    }
}

impl From<reqwest::Error> for UploadError {
    fn from(e: reqwest::Error) -> Self {
        UploadError::Reqwest(e)
    }
}

/// Upload `path` to `<vault_url>/files/` via tus, sending `Cookie: vv_session=<token>`.
/// On success, returns Ok(()). The vault file UUID is NOT returned (Sub-project C
/// will add a server-side X-Vault-File-Id header for that).
///
/// `folder_id`: optional VV folder UUID; server validates and falls back
/// to user home folder on invalid UUID.
/// `tags`: optional pre-normalized tag list; server attaches each one.
pub fn upload_file(
    vault_url: &str,
    session_token: &str,
    path: &Path,
    folder_id: Option<&str>,
    tags: &[String],
) -> Result<(), UploadError> {
    let metadata = std::fs::metadata(path)?;
    let size = metadata.len();
    let filename = path.file_name().and_then(|s| s.to_str()).ok_or_else(|| {
        UploadError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "filename not valid utf-8",
        ))
    })?;

    let upload_url = build_files_url(vault_url);
    let cookie = format!("vv_session={}", session_token);
    let metadata_header = build_upload_metadata(filename, folder_id, tags);

    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(60 * 30))
        .build()?;

    // POST to create the upload.
    let resp = client
        .post(&upload_url)
        .header("Cookie", &cookie)
        .header("Tus-Resumable", TUS_RESUMABLE)
        .header("Upload-Length", size.to_string())
        .header("Upload-Metadata", &metadata_header)
        .send()?;

    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        return Err(UploadError::Unauthorized);
    }
    if status == reqwest::StatusCode::PAYLOAD_TOO_LARGE {
        return Err(UploadError::TooLarge);
    }
    if !status.is_success() {
        return Err(UploadError::BadStatus(status.as_u16()));
    }

    let location = resp
        .headers()
        .get("Location")
        .and_then(|v| v.to_str().ok())
        .ok_or(UploadError::NoLocationHeader)?
        .to_string();

    // PATCH chunks until we've sent all bytes.
    let mut file = std::fs::File::open(path)?;
    let mut offset: u64 = 0;
    let mut buf = vec![0u8; PATCH_CHUNK_SIZE];

    while offset < size {
        use std::io::Read;
        let to_read = ((size - offset).min(PATCH_CHUNK_SIZE as u64)) as usize;
        let n = file.read(&mut buf[..to_read])?;
        if n == 0 {
            break;
        }
        let chunk = buf[..n].to_vec();

        let resp = client
            .patch(&location)
            .header("Cookie", &cookie)
            .header("Tus-Resumable", TUS_RESUMABLE)
            .header("Upload-Offset", offset.to_string())
            .header("Content-Type", "application/offset+octet-stream")
            .body(chunk)
            .send()?;

        let status = resp.status();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(UploadError::Unauthorized);
        }
        if !status.is_success() {
            return Err(UploadError::BadStatus(status.as_u16()));
        }
        offset += n as u64;
    }

    Ok(())
}

/// Build the tus collection URL: `<vault_url>/files/`.
pub fn build_files_url(vault_url: &str) -> String {
    format!("{}/files/", vault_url.trim_end_matches('/'))
}

/// Build the `Upload-Metadata` header value per tus spec: space-separated
/// `key base64(value)` pairs. `filename` is always included; `folder_id`
/// and `tags` are appended only when present/non-empty.
pub fn build_upload_metadata(
    filename: &str,
    folder_id: Option<&str>,
    tags: &[String],
) -> String {
    let mut parts = vec![format!("filename {}", STANDARD.encode(filename.as_bytes()))];
    if let Some(id) = folder_id {
        parts.push(format!("folderId {}", STANDARD.encode(id.as_bytes())));
    }
    if !tags.is_empty() {
        let joined = tags.join(",");
        parts.push(format!("tags {}", STANDARD.encode(joined.as_bytes())));
    }
    parts.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_files_url_appends_files_slash() {
        assert_eq!(
            build_files_url("https://vault.example.com"),
            "https://vault.example.com/files/"
        );
    }

    #[test]
    fn build_files_url_strips_trailing_slash_first() {
        assert_eq!(
            build_files_url("https://vault.example.com/"),
            "https://vault.example.com/files/"
        );
    }

    #[test]
    fn metadata_filename_only_when_no_extras() {
        let m = build_upload_metadata("foo.mp4", None, &[]);
        assert_eq!(m, "filename Zm9vLm1wNA==");
    }

    #[test]
    fn metadata_includes_folder_id_when_set() {
        let m = build_upload_metadata(
            "foo.mp4",
            Some("aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa"),
            &[],
        );
        // base64("aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa") =
        // "YWFhYWFhYWEtYWFhYS1hYWFhLWFhYWEtYWFhYWFhYWFhYWFh"
        assert_eq!(
            m,
            "filename Zm9vLm1wNA== folderId YWFhYWFhYWEtYWFhYS1hYWFhLWFhYWEtYWFhYWFhYWFhYWFh"
        );
    }

    #[test]
    fn metadata_includes_tags_when_non_empty() {
        let m = build_upload_metadata("foo.mp4", None, &["apex".to_string(), "clips".to_string()]);
        // base64("apex,clips") = "YXBleCxjbGlwcw=="
        assert_eq!(m, "filename Zm9vLm1wNA== tags YXBleCxjbGlwcw==");
    }

    #[test]
    fn metadata_includes_both_folder_and_tags() {
        let m = build_upload_metadata(
            "foo.mp4",
            Some("aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa"),
            &["apex".to_string()],
        );
        // base64("apex") = "YXBleA=="
        assert_eq!(
            m,
            "filename Zm9vLm1wNA== folderId YWFhYWFhYWEtYWFhYS1hYWFhLWFhYWEtYWFhYWFhYWFhYWFh tags YXBleA=="
        );
    }

    #[test]
    fn metadata_omits_tags_when_empty() {
        let m = build_upload_metadata("foo.mp4", Some("xx"), &[]);
        // base64("xx") = "eHg="
        assert_eq!(m, "filename Zm9vLm1wNA== folderId eHg=");
    }

    #[test]
    fn metadata_handles_unicode_filename() {
        let m = build_upload_metadata("café.png", None, &[]);
        // base64("café.png") = "Y2Fmw6kucG5n"
        assert_eq!(m, "filename Y2Fmw6kucG5n");
    }
}
