//! Index manifest for retrieval consistency checks.
//!
//! The manifest is used to detect whether the workspace has changed since the
//! last index build. When a mismatch is detected, callers can selectively
//! patch recall by searching only changed paths (e.g. via ripgrep), avoiding
//! full rescans or full rebuilds.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Current manifest schema version.
///
/// Bump when the manifest format changes in a backward-incompatible way.
pub const MANIFEST_SCHEMA_VERSION: u32 = 1;

/// Lightweight file fingerprint used for cheap change detection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileFingerprint {
    /// File size in bytes.
    pub size_bytes: u64,
    /// Last-modified timestamp in milliseconds since Unix epoch.
    pub modified_ms: u64,
    /// Optional strong content hash (hex).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,
}

/// Retrieval index manifest: versioning + per-path fingerprints.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexManifest {
    /// Manifest format version.
    pub schema_version: u32,
    /// Logical index version controlled by the indexing pipeline.
    pub index_version: u32,
    /// Workspace-relative path -> fingerprint.
    pub entries: HashMap<String, FileFingerprint>,
}

impl IndexManifest {
    /// Load manifest from disk. Returns `None` if missing or invalid.
    pub fn load(path: &Path) -> Option<Self> {
        let raw = std::fs::read_to_string(path).ok()?;
        let manifest: Self = serde_json::from_str(&raw).ok()?;
        if manifest.schema_version != MANIFEST_SCHEMA_VERSION {
            return None;
        }
        Some(manifest)
    }

    /// Resolve workspace-relative manifest path.
    pub fn resolve_path(workspace_dir: &Path, manifest_rel_path: &str) -> PathBuf {
        let rel = manifest_rel_path.trim();
        if rel.is_empty() {
            return workspace_dir.join("state/retrieval-index.json");
        }
        let p = PathBuf::from(rel);
        if p.is_absolute() {
            p
        } else {
            workspace_dir.join(p)
        }
    }
}

/// Compute a lightweight fingerprint from filesystem metadata.
pub fn fingerprint_for_path(path: &Path) -> Option<FileFingerprint> {
    let meta = std::fs::metadata(path).ok()?;
    let modified = meta.modified().ok()?;
    let modified_ms = modified
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|d| d.as_millis())
        .and_then(|ms| u64::try_from(ms).ok())?;
    Some(FileFingerprint {
        size_bytes: meta.len(),
        modified_ms,
        content_hash: None,
    })
}

/// Return workspace-relative paths that changed since the manifest snapshot.
///
/// Change detection is cheap: compares size + mtime. Missing files are treated as changed.
pub fn changed_paths(
    workspace_dir: &Path,
    manifest: &IndexManifest,
    expected_index_version: Option<u32>,
    max_paths: usize,
) -> Vec<String> {
    // If caller expects a specific index version and the manifest differs,
    // treat everything in manifest as potentially stale (but still bounded).
    let version_mismatch = expected_index_version
        .map(|v| v != manifest.index_version)
        .unwrap_or(false);

    let mut changed = Vec::new();
    let limit = max_paths.max(1);

    for (rel, snap) in &manifest.entries {
        if changed.len() >= limit {
            break;
        }

        if version_mismatch {
            changed.push(rel.clone());
            continue;
        }

        let abs = workspace_dir.join(rel);
        match fingerprint_for_path(&abs) {
            Some(current) => {
                if current.size_bytes != snap.size_bytes || current.modified_ms != snap.modified_ms {
                    changed.push(rel.clone());
                }
            }
            None => {
                changed.push(rel.clone());
            }
        }
    }

    changed
}

