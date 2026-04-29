//! HTTP proxies that the settings UI calls (via Tauri commands) to fetch
//! data from the VV server using the keychain session cookie. Pure logic
//! for breadcrumb computation lives here too so it can be unit-tested.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::Duration;

#[derive(Debug, Clone, Deserialize)]
struct FolderRow {
    pub id: String,
    pub name: String,
    pub parent_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FolderNode {
    pub id: String,
    pub name: String,
    pub parent_id: Option<String>,
    /// Breadcrumb display: "Games / Apex" for nested, "Apex" for root-level.
    pub breadcrumb: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TagSuggestion {
    pub name: String,
    pub file_count: u32,
}

#[derive(Debug)]
pub enum FetchError {
    Network(String),
    Status(u16),
    Unauthorized,
    Decode(String),
}

impl std::fmt::Display for FetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FetchError::Network(s) => write!(f, "network: {}", s),
            FetchError::Status(s) => write!(f, "bad status: {}", s),
            FetchError::Unauthorized => write!(f, "session expired"),
            FetchError::Decode(s) => write!(f, "decode: {}", s),
        }
    }
}

impl std::error::Error for FetchError {}

/// Compute breadcrumb labels ("Games / Apex") for each folder by walking
/// `parent_id` up to the root. Pure function, easily testable.
fn compute_breadcrumbs(rows: &[FolderRow]) -> Vec<FolderNode> {
    let by_id: HashMap<String, &FolderRow> =
        rows.iter().map(|r| (r.id.clone(), r)).collect();

    rows.iter()
        .map(|r| {
            let mut chain: Vec<String> = vec![r.name.clone()];
            let mut current_parent = r.parent_id.clone();
            while let Some(pid) = current_parent {
                if let Some(parent) = by_id.get(&pid) {
                    chain.push(parent.name.clone());
                    current_parent = parent.parent_id.clone();
                } else {
                    break;
                }
            }
            chain.reverse();
            FolderNode {
                id: r.id.clone(),
                name: r.name.clone(),
                parent_id: r.parent_id.clone(),
                breadcrumb: chain.join(" / "),
            }
        })
        .collect()
}

pub fn fetch_folders(vault_url: &str, session_token: &str) -> Result<Vec<FolderNode>, FetchError> {
    let url = format!("{}/api/folders/tree", vault_url.trim_end_matches('/'));
    let cookie = format!("vv_session={}", session_token);
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| FetchError::Network(e.to_string()))?;

    let resp = client
        .get(&url)
        .header("Cookie", &cookie)
        .send()
        .map_err(|e| FetchError::Network(e.to_string()))?;

    if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
        return Err(FetchError::Unauthorized);
    }
    if !resp.status().is_success() {
        return Err(FetchError::Status(resp.status().as_u16()));
    }

    #[derive(Deserialize)]
    struct Wrapper {
        folders: Vec<FolderRow>,
    }

    let wrapper: Wrapper = resp.json().map_err(|e| FetchError::Decode(e.to_string()))?;
    Ok(compute_breadcrumbs(&wrapper.folders))
}

pub fn fetch_tags(vault_url: &str, session_token: &str) -> Result<Vec<TagSuggestion>, FetchError> {
    let url = format!("{}/api/tags", vault_url.trim_end_matches('/'));
    let cookie = format!("vv_session={}", session_token);
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| FetchError::Network(e.to_string()))?;

    let resp = client
        .get(&url)
        .header("Cookie", &cookie)
        .send()
        .map_err(|e| FetchError::Network(e.to_string()))?;

    if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
        return Err(FetchError::Unauthorized);
    }
    if !resp.status().is_success() {
        return Err(FetchError::Status(resp.status().as_u16()));
    }

    #[derive(Deserialize)]
    struct ServerTag {
        name: String,
        file_count: u32,
    }
    #[derive(Deserialize)]
    struct Wrapper {
        tags: Vec<ServerTag>,
    }

    let wrapper: Wrapper = resp.json().map_err(|e| FetchError::Decode(e.to_string()))?;
    Ok(wrapper
        .tags
        .into_iter()
        .map(|t| TagSuggestion {
            name: t.name,
            file_count: t.file_count,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: &str, name: &str, parent: Option<&str>) -> FolderRow {
        FolderRow {
            id: id.to_string(),
            name: name.to_string(),
            parent_id: parent.map(|s| s.to_string()),
        }
    }

    #[test]
    fn breadcrumb_for_root_folder_is_just_the_name() {
        let rows = vec![row("1", "Apex", None)];
        let out = compute_breadcrumbs(&rows);
        assert_eq!(out[0].breadcrumb, "Apex");
    }

    #[test]
    fn breadcrumb_for_nested_folder_includes_chain() {
        let rows = vec![
            row("1", "Games", None),
            row("2", "Apex", Some("1")),
            row("3", "Highlights", Some("2")),
        ];
        let out = compute_breadcrumbs(&rows);
        let by_id: std::collections::HashMap<_, _> =
            out.iter().map(|f| (f.id.as_str(), f.breadcrumb.as_str())).collect();
        assert_eq!(by_id["1"], "Games");
        assert_eq!(by_id["2"], "Games / Apex");
        assert_eq!(by_id["3"], "Games / Apex / Highlights");
    }

    #[test]
    fn breadcrumb_handles_orphan_folders() {
        // Parent ID points at a non-existent folder (data inconsistency edge case).
        let rows = vec![row("1", "Stranded", Some("does-not-exist"))];
        let out = compute_breadcrumbs(&rows);
        assert_eq!(out[0].breadcrumb, "Stranded");
    }

    #[test]
    fn breadcrumb_preserves_input_order() {
        let rows = vec![
            row("z", "Zebra", None),
            row("a", "Alpha", None),
        ];
        let out = compute_breadcrumbs(&rows);
        assert_eq!(out[0].id, "z");
        assert_eq!(out[1].id, "a");
    }
}
