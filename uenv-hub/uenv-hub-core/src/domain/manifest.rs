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
