//! Consume a locally-synced Hub EnvPackage (the output of `uenv env sync`).
//!
//! A synced package directory (`<target>/envs/<package>/<version>/`) contains:
//!   - `manifest.json`           — the full `EnvPackageManifest` (worker_overlay etc.)
//!   - `catalog.json`            — instance catalog (same shape `InstanceStore` reads)
//!   - `images.manifest.json`    — digest-locked image index for `local_only` checks
//!   - `worker.overlay.yaml`     — overlay for ops (JSON content, valid YAML)
//!   - `.synced`                 — bundle digest marker
//!
//! The Worker reads the catalog + overlay from here so it can run from a
//! pre-provisioned environment without re-pulling a catalog (or images) from
//! third parties (design 260629-hub-env-package-design.md §5.1 / §8.1).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::swe::image_cache::ImagePullPolicy;

/// One image entry from `images.manifest.json`.
#[derive(Debug, Clone)]
pub struct ImageEntry {
    pub image: String,
    /// `sha256:...` digest, or empty when the package did not pin one.
    pub digest: String,
}

/// A locally-synced EnvPackage directory.
#[derive(Debug, Clone)]
pub struct EnvPackageDir {
    pub dir: PathBuf,
    pub package_id: String,
    pub version: String,
    /// `worker_overlay.swe.benchmark_variant`, if declared.
    pub variant: Option<String>,
    /// `worker_overlay.swe.image_pull_policy`, if declared.
    pub image_pull_policy: Option<ImagePullPolicy>,
    /// Path to the bundled `catalog.json`.
    pub catalog_path: PathBuf,
    /// instance_id -> image/digest from `images.manifest.json`.
    pub images: HashMap<String, ImageEntry>,
}

impl EnvPackageDir {
    /// True if `dir` looks like a synced package (manifest + catalog present).
    pub fn is_synced(dir: &Path) -> bool {
        dir.join("manifest.json").is_file() && dir.join("catalog.json").is_file()
    }

    /// Load and parse a synced package directory.
    pub fn load(dir: &Path) -> Result<Self, String> {
        let manifest_path = dir.join("manifest.json");
        let raw = std::fs::read_to_string(&manifest_path)
            .map_err(|e| format!("read {}: {e}", manifest_path.display()))?;
        let manifest: serde_json::Value =
            serde_json::from_str(&raw).map_err(|e| format!("parse manifest.json: {e}"))?;

        let package_id = manifest
            .get("package_id")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        let version = manifest
            .get("version")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();

        let overlay = manifest.get("worker_overlay");
        let variant = overlay
            .and_then(|o| o.pointer("/swe/benchmark_variant"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let image_pull_policy = overlay
            .and_then(|o| o.pointer("/swe/image_pull_policy"))
            .and_then(|v| v.as_str())
            .and_then(ImagePullPolicy::parse);

        let catalog_path = dir.join("catalog.json");
        if !catalog_path.is_file() {
            return Err(format!("missing catalog.json in {}", dir.display()));
        }

        let images = load_images_manifest(&dir.join("images.manifest.json"));

        Ok(Self {
            dir: dir.to_path_buf(),
            package_id,
            version,
            variant,
            image_pull_policy,
            catalog_path,
            images,
        })
    }
}

/// Parse `images.manifest.json` into an instance_id -> [`ImageEntry`] map.
/// Missing/invalid file yields an empty map (image gating then degrades to
/// "present-only" with no digest pinning).
fn load_images_manifest(path: &Path) -> HashMap<String, ImageEntry> {
    let mut out = HashMap::new();
    let Ok(raw) = std::fs::read_to_string(path) else {
        return out;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return out;
    };
    if let Some(arr) = value.get("images").and_then(|v| v.as_array()) {
        for e in arr {
            if let Some(id) = e.get("instance_id").and_then(|v| v.as_str()) {
                out.insert(
                    id.to_string(),
                    ImageEntry {
                        image: e
                            .get("image")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_string(),
                        digest: e
                            .get("digest")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_string(),
                    },
                );
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_manifest_overlay_and_images() {
        let dir = std::env::temp_dir().join(format!("uenv-envpkg-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("manifest.json"),
            r#"{
              "package_id": "swe-bench-pro",
              "version": "0.1.0",
              "worker_overlay": {"swe": {"benchmark_variant": "pro", "image_pull_policy": "local_only"}}
            }"#,
        )
        .unwrap();
        std::fs::write(dir.join("catalog.json"), "{}").unwrap();
        std::fs::write(
            dir.join("images.manifest.json"),
            r#"{"images":[{"instance_id":"swe-pro__example-go-1","image":"registry.example.com/x:y","digest":"sha256:abc"}]}"#,
        )
        .unwrap();

        assert!(EnvPackageDir::is_synced(&dir));
        let pkg = EnvPackageDir::load(&dir).unwrap();
        assert_eq!(pkg.package_id, "swe-bench-pro");
        assert_eq!(pkg.version, "0.1.0");
        assert_eq!(pkg.variant.as_deref(), Some("pro"));
        assert_eq!(pkg.image_pull_policy, Some(ImagePullPolicy::LocalOnly));
        assert_eq!(pkg.images.get("swe-pro__example-go-1").unwrap().digest, "sha256:abc");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
