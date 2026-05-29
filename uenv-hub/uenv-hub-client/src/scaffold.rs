//! Template archive extraction for `uenv env init`.

use crate::error::{ClientError, Result};
use flate2::read::GzDecoder;
use sha2::{Digest, Sha256};
use std::path::Path;

/// Verify an archive's sha256 against an expected hex digest.
pub fn verify_sha256(bytes: &[u8], expected_hex: &str) -> bool {
    let actual = hex::encode(Sha256::digest(bytes));
    actual.eq_ignore_ascii_case(expected_hex)
}

/// Extract a `tar.gz` archive into `dest_dir`, creating it if needed.
///
/// Refuses entries that would escape `dest_dir` (path traversal guard).
pub fn extract_targz(bytes: &[u8], dest_dir: &Path) -> Result<Vec<String>> {
    std::fs::create_dir_all(dest_dir)?;
    let dest_canon = dest_dir
        .canonicalize()
        .map_err(|e| ClientError::Io(format!("canonicalize dest: {e}")))?;

    let decoder = GzDecoder::new(bytes);
    let mut archive = tar::Archive::new(decoder);
    let mut written = Vec::new();

    for entry in archive
        .entries()
        .map_err(|e| ClientError::Other(format!("reading archive: {e}")))?
    {
        let mut entry = entry.map_err(|e| ClientError::Other(format!("archive entry: {e}")))?;
        let path = entry
            .path()
            .map_err(|e| ClientError::Other(format!("entry path: {e}")))?
            .into_owned();

        let out_path = dest_dir.join(&path);
        // Guard against path traversal (../ etc.).
        if let Some(parent) = out_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let normalized = out_path
            .canonicalize()
            .ok()
            .or_else(|| out_path.parent().and_then(|p| p.canonicalize().ok()));
        if let Some(n) = normalized {
            if !n.starts_with(&dest_canon) {
                return Err(ClientError::Other(format!(
                    "refusing to extract outside destination: {}",
                    path.display()
                )));
            }
        }

        entry
            .unpack(&out_path)
            .map_err(|e| ClientError::Io(format!("unpacking {}: {e}", path.display())))?;
        written.push(path.display().to_string());
    }
    Ok(written)
}
