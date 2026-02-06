//! Local on-disk persistence for MCP tokens and client IDs.
//!
//! Tokens and client IDs obtained during OAuth are cached in a small JSON
//! file under the platform config directory so they survive restarts even
//! when Rice is unavailable.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};

use crate::constants::APP_NAME;

/// Locally cached MCP credentials (tokens, client IDs, refresh tokens).
#[derive(Default, Serialize, Deserialize, Clone)]
pub struct LocalMcpStore {
    pub tokens: HashMap<String, String>,
    pub client_ids: HashMap<String, String>,
    pub refresh_tokens: HashMap<String, String>,
}

/// Returns the platform-specific path for the local MCP store file.
fn local_store_path() -> Option<PathBuf> {
    ProjectDirs::from("com", APP_NAME, APP_NAME)
        .map(|dirs| dirs.config_dir().join("local_mcp_store.json"))
}

/// Load the local MCP store from disk, falling back to an empty store.
pub fn load_local_mcp_store() -> LocalMcpStore {
    let Some(path) = local_store_path() else {
        return LocalMcpStore::default();
    };
    let Ok(contents) = fs::read_to_string(&path) else {
        return LocalMcpStore::default();
    };
    serde_json::from_str(&contents).unwrap_or_default()
}

/// Persist the local MCP store to disk.
pub fn persist_local_mcp_store(store: &LocalMcpStore) -> Result<()> {
    let Some(path) = local_store_path() else {
        return Ok(());
    };
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).context("create config dir")?;
    }
    let contents = serde_json::to_string_pretty(store).context("serialize local mcp store")?;
    fs::write(&path, contents).context("write local mcp store")?;
    Ok(())
}
