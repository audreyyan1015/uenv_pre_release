//! JSON Schema validation built on the `jsonschema` crate.
//!
//! Used to (a) verify that a submitted `config_schema` is itself a usable JSON
//! Schema, (b) verify that `default_config` validates against that schema, and
//! (c) validate the Action/Observation/State interface schemas. Results are
//! reported as a shared [`ValidationReport`] so the CLI and server agree.

use serde_json::Value;
use uenv_hub_types::ValidationReport;

/// Compile `schema` and return whether it is a structurally valid JSON Schema.
///
/// `location` is used as the issue location prefix (e.g. `config_schema`).
pub fn check_is_schema(schema: &Value, location: &str, report: &mut ValidationReport) {
    if let Err(e) = jsonschema::JSONSchema::compile(schema) {
        report.push_error(location, format!("not a valid JSON Schema: {e}"));
    }
}

/// Validate `instance` against `schema`, recording every violation.
pub fn validate_instance(
    schema: &Value,
    instance: &Value,
    location: &str,
    report: &mut ValidationReport,
) {
    let compiled = match jsonschema::JSONSchema::compile(schema) {
        Ok(c) => c,
        Err(e) => {
            report.push_error(location, format!("schema does not compile: {e}"));
            return;
        }
    };
    if let Err(errors) = compiled.validate(instance) {
        for err in errors {
            let path = err.instance_path.to_string();
            let loc = if path.is_empty() {
                location.to_string()
            } else {
                format!("{location}{path}")
            };
            report.push_error(loc, err.to_string());
        }
    }
}

/// Convenience: validate `instance` and return a boolean (no detailed report).
pub fn is_valid(schema: &Value, instance: &Value) -> bool {
    match jsonschema::JSONSchema::compile(schema) {
        Ok(c) => c.is_valid(instance),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn valid_schema_accepts_matching_instance() {
        let schema = json!({"type": "object", "properties": {"x": {"type": "string"}}, "required": ["x"]});
        let mut report = ValidationReport::ok();
        validate_instance(&schema, &json!({"x": "hi"}), "default_config", &mut report);
        assert!(report.valid, "{:?}", report.issues);
    }

    #[test]
    fn invalid_instance_reports_issue() {
        let schema = json!({"type": "object", "properties": {"x": {"type": "string"}}, "required": ["x"]});
        let mut report = ValidationReport::ok();
        validate_instance(&schema, &json!({}), "default_config", &mut report);
        assert!(!report.valid);
        assert_eq!(report.issues.len(), 1);
    }

    #[test]
    fn broken_schema_detected() {
        let schema = json!({"type": "not-a-type"});
        let mut report = ValidationReport::ok();
        check_is_schema(&schema, "config_schema", &mut report);
        assert!(!report.valid);
    }
}
