//! Manifest-level domain rules: structural validation, namespace ownership,
//! yank rules and dependency-reference checks.
//!
//! This is pure logic (no DB) so it can run identically in the CLI's local
//! `validate` command and in the server before a publish hits the repository.

use crate::domain::{interface, version};
use crate::schema_validator;
use uenv_hub_types::{
    InterfaceSchema, PublishVersionRequest, Role, TokenInfo, ValidationReport,
};

/// Validate `env_type` naming: lowercase alphanumerics, `-`, `_`, `.`.
pub fn validate_env_type(env_type: &str, report: &mut ValidationReport) {
    if env_type.is_empty() {
        report.push_error("env_type", "must not be empty");
        return;
    }
    if env_type.len() > 128 {
        report.push_error("env_type", "must be at most 128 characters");
    }
    let ok = env_type
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '_' | '.'));
    if !ok {
        report.push_error(
            "env_type",
            "may only contain lowercase letters, digits, '-', '_', '.'",
        );
    }
}

/// Well-known **public** container registries. Referencing any of these means a
/// worker would pull from the internet at runtime, which breaks the intranet
/// "zero external pull" (零外拉) guarantee. Detection is host-exact to avoid
/// false positives on internal registries (`registry.local`, private IPs) and
/// bare local names (`uenv-base:latest`).
const PUBLIC_REGISTRIES: &[&str] = &[
    "docker.io",
    "registry-1.docker.io",
    "index.docker.io",
    "ghcr.io",
    "quay.io",
    "gcr.io",
    "k8s.gcr.io",
    "registry.k8s.io",
    "public.ecr.aws",
    "mcr.microsoft.com",
    "nvcr.io",
    "docker.elastic.co",
];

/// If `image_ref`'s registry host is a known public registry, return it. Only
/// the explicit host component (text before the first `/`) is matched, so
/// `registry.local/uenv/x:1` and `uenv-base:latest` pass while
/// `docker.io/library/python:3.11` is flagged.
pub fn public_registry_of(image_ref: &str) -> Option<&'static str> {
    let host = image_ref.trim().split('/').next().unwrap_or("");
    PUBLIC_REGISTRIES
        .iter()
        .copied()
        .find(|reg| host.eq_ignore_ascii_case(reg))
}

/// Full structural validation of a publish request. Does not touch the DB.
pub fn validate_publish(req: &PublishVersionRequest) -> ValidationReport {
    let mut report = ValidationReport::ok();

    // Version must be valid semver.
    if version::parse(&req.version).is_err() {
        report.push_error("version", "not a valid semantic version");
    }

    // Image digest, when present, should look like sha256:...
    if let Some(image) = &req.image {
        if image.url.trim().is_empty() {
            report.push_error("image.url", "must not be empty");
        }
        if let Some(digest) = &image.digest {
            if !digest.starts_with("sha256:") || digest.len() < "sha256:".len() + 8 {
                report.push_warning(
                    "image.digest",
                    "expected a 'sha256:<hex>' digest for tamper protection",
                );
            }
        }
        // Zero-egress: the runtime image is what a worker pulls; a public
        // registry reference here defeats intranet deployment.
        if let Some(reg) = public_registry_of(&image.url) {
            report.push_warning(
                "image.url",
                format!(
                    "references public registry '{reg}'; for 内网零外拉 host the image on the Hub \
                     (`uenv env publish-image`) or an internal registry and reference that instead"
                ),
            );
        }
    }

    // Zero-egress: build-time base images should also resolve internally. These
    // are staged during the air-gap offline build, so this is a guidance
    // warning rather than a hard error.
    for (loc, base) in [
        ("version.base_image", req.base_image.as_deref()),
        (
            "image.base_image_ref",
            req.image.as_ref().and_then(|i| i.base_image_ref.as_deref()),
        ),
    ] {
        if let Some(reg) = base.and_then(public_registry_of) {
            report.push_warning(
                loc,
                format!(
                    "base image references public registry '{reg}'; mirror it internally (air-gap \
                     offline build) so the image build never pulls from the internet"
                ),
            );
        }
    }

    // OpenEnv standardization: a fully-standardized environment declares the
    // Action / Observation / State contract and a launch entrypoint. Missing
    // pieces are surfaced as guidance (non-fatal) so authors converge on the
    // standard without blocking early scaffolds.
    if req.interface.action.is_none() {
        report.push_warning(
            "interface.action",
            "standardized environments should declare an OpenEnv `action` JSON Schema",
        );
    }
    if req.interface.observation.is_none() {
        report.push_warning(
            "interface.observation",
            "standardized environments should declare an OpenEnv `observation` JSON Schema",
        );
    }
    if req.interface.state.is_none() {
        report.push_warning(
            "interface.state",
            "standardized environments should declare an OpenEnv `state` JSON Schema",
        );
    }
    if req.entrypoint.as_deref().unwrap_or("").trim().is_empty() && req.image.is_none() {
        report.push_warning(
            "entrypoint",
            "declare a version.entrypoint or an [image] so the worker knows how to launch the environment",
        );
    }

    // health_check_path should start with '/'.
    if let Some(path) = &req.health_check_path {
        if !path.starts_with('/') {
            report.push_warning("health_check_path", "should start with '/'");
        }
    }

    // config_schema must itself be a JSON Schema; default_config must validate.
    if let Some(schema) = &req.config_schema {
        schema_validator::check_is_schema(schema, "config_schema", &mut report);
        if let Some(default) = &req.default_config {
            schema_validator::validate_instance(schema, default, "default_config", &mut report);
        }
    }

    // Interface schemas.
    interface::validate_interface(&req.interface, &mut report);

    // Examples: validate embedded actions against the action schema.
    for (i, example) in req.examples.iter().enumerate() {
        interface::validate_example_actions(
            &req.interface,
            &example.request,
            &format!("examples[{i}]"),
            &mut report,
        );
    }

    // Dependency references must be `env_type@version` (or `env_type@^range`).
    if let Some(deps) = &req.dependencies {
        for (i, dep) in deps.requires.iter().enumerate() {
            if !dep.contains('@') {
                report.push_error(
                    format!("dependencies.requires[{i}]"),
                    "must be of the form 'env_type@version'",
                );
            }
        }
    }

    report
}

/// Validate the manifest of an env-type init scaffold (CLI side). Accepts the
/// env_type plus the publish-shaped request.
pub fn validate_manifest(env_type: &str, req: &PublishVersionRequest) -> ValidationReport {
    let mut report = ValidationReport::ok();
    validate_env_type(env_type, &mut report);
    report.merge(validate_publish(req));
    report
}

/// Check whether `principal` may publish/modify the given `namespace`.
///
/// * admins may touch any namespace,
/// * publishers may only touch namespaces in their allow-list (empty allow-list
///   means "default namespace only"),
/// * readers may not modify anything.
pub fn can_write_namespace(principal: &TokenInfo, namespace: &str) -> bool {
    match principal.role {
        Role::Admin => true,
        Role::Publisher => {
            if principal.namespaces.is_empty() {
                namespace == "default"
            } else {
                principal
                    .namespaces
                    .iter()
                    .any(|n| n == namespace || n == "*")
            }
        }
        Role::Reader => false,
    }
}

/// Validate a yank request: a reason is required and must be non-trivial.
pub fn validate_yank_reason(reason: &str) -> ValidationReport {
    let mut report = ValidationReport::ok();
    if reason.trim().len() < 3 {
        report.push_error("reason", "a yank reason of at least 3 characters is required");
    }
    report
}

/// Re-export so callers can validate just the interface block.
pub fn validate_interface_only(interface: &InterfaceSchema) -> ValidationReport {
    let mut report = ValidationReport::ok();
    interface::validate_interface(interface, &mut report);
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use uenv_hub_types::{ImageSpec, InterfaceSchema};

    fn base_req() -> PublishVersionRequest {
        PublishVersionRequest {
            version: "1.0.0".into(),
            changelog: None,
            image: None,
            base_image: None,
            health_check_path: Some("/health".into()),
            entrypoint: None,
            supported_backends: vec!["process".into()],
            config_schema: None,
            default_config: None,
            resources: Default::default(),
            interface: InterfaceSchema::default(),
            examples: vec![],
            dependencies: None,
            min_uenv_version: None,
        }
    }

    #[test]
    fn good_request_validates() {
        let report = validate_publish(&base_req());
        assert!(report.valid, "{:?}", report.issues);
    }

    #[test]
    fn bad_version_rejected() {
        let mut req = base_req();
        req.version = "not-semver".into();
        assert!(!validate_publish(&req).valid);
    }

    #[test]
    fn default_config_must_match_schema() {
        let mut req = base_req();
        req.config_schema = Some(json!({"type": "object", "required": ["x"]}));
        req.default_config = Some(json!({}));
        assert!(!validate_publish(&req).valid);
    }

    #[test]
    fn public_registry_detection() {
        assert_eq!(public_registry_of("docker.io/library/python:3.11"), Some("docker.io"));
        assert_eq!(public_registry_of("ghcr.io/org/img:1"), Some("ghcr.io"));
        assert_eq!(public_registry_of("Quay.io/x:2"), Some("quay.io"));
        // Internal / local references pass.
        assert_eq!(public_registry_of("registry.local/uenv/math:0.2.0"), None);
        assert_eq!(public_registry_of("uenv-base:latest"), None);
        assert_eq!(public_registry_of("10.0.0.5:5000/uenv/x:1"), None);
    }

    #[test]
    fn public_registry_image_is_warned_not_failed() {
        let mut req = base_req();
        req.image = Some(ImageSpec {
            url: "docker.io/library/python:3.11".into(),
            digest: None,
            size_bytes: None,
            arch: None,
            base_image_ref: None,
        });
        let report = validate_publish(&req);
        assert!(report.valid, "zero-egress lint must be a warning, not an error");
        assert!(report
            .issues
            .iter()
            .any(|i| i.location == "image.url" && i.message.contains("public registry")));
    }

    #[test]
    fn internal_registry_image_has_no_zero_egress_warning() {
        let mut req = base_req();
        req.image = Some(ImageSpec {
            url: "registry.local/uenv/math:0.2.0".into(),
            digest: None,
            size_bytes: None,
            arch: None,
            base_image_ref: Some("uenv-base:latest".into()),
        });
        let report = validate_publish(&req);
        assert!(report.valid);
        assert!(!report
            .issues
            .iter()
            .any(|i| i.message.contains("public registry")));
    }

    #[test]
    fn missing_openenv_interface_is_guidance_only() {
        // base_req has no interface schemas → three guidance warnings, still valid.
        let report = validate_publish(&base_req());
        assert!(report.valid);
        for loc in ["interface.action", "interface.observation", "interface.state"] {
            assert!(
                report.issues.iter().any(|i| i.location == loc),
                "expected guidance warning at {loc}"
            );
        }
    }

    #[test]
    fn digest_warning_does_not_fail() {
        let mut req = base_req();
        req.image = Some(ImageSpec {
            url: "registry.io/x:1".into(),
            digest: Some("deadbeef".into()),
            size_bytes: None,
            arch: None,
            base_image_ref: None,
        });
        let report = validate_publish(&req);
        assert!(report.valid); // warning only
        assert!(!report.issues.is_empty());
    }

    #[test]
    fn namespace_rbac() {
        let admin = TokenInfo {
            id: 1,
            name: "a".into(),
            owner: None,
            role: Role::Admin,
            namespaces: vec![],
        };
        assert!(can_write_namespace(&admin, "anything"));

        let pub_default = TokenInfo {
            id: 2,
            name: "p".into(),
            owner: None,
            role: Role::Publisher,
            namespaces: vec![],
        };
        assert!(can_write_namespace(&pub_default, "default"));
        assert!(!can_write_namespace(&pub_default, "team-x"));

        let reader = TokenInfo {
            id: 3,
            name: "r".into(),
            owner: None,
            role: Role::Reader,
            namespaces: vec![],
        };
        assert!(!can_write_namespace(&reader, "default"));
    }
}
