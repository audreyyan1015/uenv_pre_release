//! Interface schema parsing & validation (OpenEnv-style strong typing).
//!
//! An environment declares three JSON Schemas — `action`, `observation`,
//! `state` — describing its strongly-typed contract. This module checks that
//! each provided schema is itself a valid JSON Schema. The resulting
//! [`ValidationReport`] is reused by both the CLI (`uenv env validate`) and the
//! server publish path so the two never disagree.

use crate::schema_validator;
use uenv_hub_types::{InterfaceSchema, ValidationReport};

/// Validate the three interface schemas. Each is optional, but any that is
/// present must compile as a JSON Schema.
pub fn validate_interface(interface: &InterfaceSchema, report: &mut ValidationReport) {
    if let Some(action) = &interface.action {
        schema_validator::check_is_schema(action, "interface.action", report);
    }
    if let Some(observation) = &interface.observation {
        schema_validator::check_is_schema(observation, "interface.observation", report);
    }
    if let Some(state) = &interface.state {
        schema_validator::check_is_schema(state, "interface.state", report);
    }
}

/// Validate a single example `EpisodeRequest` against the action schema, if the
/// example exposes an `actions` array and an action schema is present.
pub fn validate_example_actions(
    interface: &InterfaceSchema,
    example_request: &serde_json::Value,
    location: &str,
    report: &mut ValidationReport,
) {
    let Some(action_schema) = &interface.action else {
        return;
    };
    let Some(actions) = example_request.get("actions").and_then(|a| a.as_array()) else {
        return;
    };
    for (i, action) in actions.iter().enumerate() {
        schema_validator::validate_instance(
            action_schema,
            action,
            &format!("{location}.actions[{i}]"),
            report,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn valid_interface_passes() {
        let iface = InterfaceSchema {
            action: Some(json!({"type": "object", "properties": {"answer": {"type": "string"}}})),
            observation: Some(json!({"type": "object"})),
            state: None,
        };
        let mut report = ValidationReport::ok();
        validate_interface(&iface, &mut report);
        assert!(report.valid, "{:?}", report.issues);
    }

    #[test]
    fn broken_interface_reported() {
        let iface = InterfaceSchema {
            action: Some(json!({"type": 123})),
            ..Default::default()
        };
        let mut report = ValidationReport::ok();
        validate_interface(&iface, &mut report);
        assert!(!report.valid);
    }
}
