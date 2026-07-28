//! Episode Stack composition rules.
//!
//! A Task Environment says what one `reset/step` pair means. An Episode Stack
//! says what actually runs an episode: that environment, plus the Agent scaffold
//! that decides how an answer gets written, plus — on the SWE path — the Runtime
//! Gateway session that routes the scaffold's terminal commands into the
//! Worker-side container.
//!
//! Two kinds of checks live here, and the split matters:
//!
//! 1. [`validate`] is **structural** — pure function of the request, no I/O. It
//!    catches internal contradictions such as declaring `execution_mode = agent`
//!    with no scaffold.
//! 2. [`cross_check`] is **referential** — it compares the request against what
//!    the Hub actually holds. It catches the pairing mistakes that structural
//!    validation cannot see: a scaffold that drives `swe` bolted onto the `code`
//!    environment, or an agent-mode SWE stack that forgot the gateway.
//!
//! The second class is the reason this table exists at all. Both mistakes above
//! were previously only discoverable at dispatch time, and the second one is the
//! precise shape of the SWE-bench defect this round fixed: the scaffold ran its
//! commands locally instead of through the gateway, so no task could pass while
//! every component individually looked correctly configured.

use uenv_hub_types::{self as dto, ValidationReport};

/// `env_type`s whose Agent path cannot work without a Runtime Gateway session,
/// because the scaffold and the environment live on different hosts.
const GATEWAY_BOUND_ENV_TYPES: &[&str] = &["swe", "swebenchpro", "swebench"];

/// Facts about a referenced Agent scaffold, read from the Hub.
#[derive(Debug, Clone, Default)]
pub struct ScaffoldFacts {
    pub resolved_version: String,
    pub agent_kind: Option<String>,
    /// `agent_defaults.required_env_types` as published.
    pub required_env_types: Vec<String>,
    /// `platform.consumers` as published.
    pub consumers: Vec<String>,
}

/// Facts about the referenced Task Environment, read from the Hub.
#[derive(Debug, Clone, Default)]
pub struct TaskEnvFacts {
    pub resolved_version: String,
    /// Dataset values the version's `config_schema` accepts, when it declares any.
    pub dataset_keys: Option<Vec<String>>,
    /// Whether the resolved version may serve as `latest` (rubric gate outcome).
    pub latest_eligible: bool,
    pub lifecycle: dto::EnvLifecycle,
    pub superseded_by: Option<String>,
}

/// Validate a stack request's internal coherence. No Hub lookups.
pub fn validate(req: &dto::PublishStackRequest, report: &mut ValidationReport) {
    if req.task_env.env_type.trim().is_empty() {
        report.push_error("task_env.env_type", "must not be empty");
    }
    if req.task_env.version.trim().is_empty() {
        report.push_error(
            "task_env.version",
            "must be a semver constraint or 'latest'",
        );
    }

    match (req.execution_mode, &req.agent_scaffold) {
        (dto::ExecutionMode::Agent, None) => report.push_error(
            "agent_scaffold",
            "execution_mode is 'agent' but no scaffold is declared; there is then \
             nothing to produce actions",
        ),
        (dto::ExecutionMode::Native, Some(s)) => report.push_error(
            "agent_scaffold",
            format!(
                "execution_mode is 'native' but scaffold '{}' is declared; native means \
                 the Worker calls the model itself, so the scaffold would never run",
                s.package_id
            ),
        ),
        _ => {}
    }

    if let Some(scaffold) = &req.agent_scaffold {
        if scaffold.package_id.trim().is_empty() {
            report.push_error("agent_scaffold.package_id", "must not be empty");
        }
        if scaffold.version.trim().is_empty() {
            report.push_error(
                "agent_scaffold.version",
                "must be a semver constraint or 'latest'",
            );
        }
        if let Some(consumer) = &scaffold.consumer {
            let known = [
                dto::CONSUMER_WORKER,
                dto::CONSUMER_TOOLENV_AGENT,
                dto::CONSUMER_OPENHANDS_AGENT,
            ];
            if !known.contains(&consumer.as_str()) {
                report.push_error(
                    "agent_scaffold.consumer",
                    format!("unknown consumer role '{consumer}'; expected one of {}", known.join(" / ")),
                );
            }
        }
    }

    if req.runtime_gateway.required && req.execution_mode == dto::ExecutionMode::Native {
        report.push_warning(
            "runtime_gateway.required",
            "a native stack has no external scaffold to route commands for; the gateway \
             requirement will have no consumer",
        );
    }

    for (i, pkg) in req.env_packages.iter().enumerate() {
        if !pkg.contains('@') {
            report.push_error(
                format!("env_packages[{i}]"),
                format!("'{pkg}' must be of the form 'package_id@version'"),
            );
        }
    }
}

/// Compare the request against what the Hub holds.
///
/// `scaffold` is `None` when the stack declares no scaffold (native mode) — the
/// caller is expected to have already reported a missing-but-required scaffold as
/// a lookup error before getting here.
pub fn cross_check(
    req: &dto::PublishStackRequest,
    env: &TaskEnvFacts,
    scaffold: Option<&ScaffoldFacts>,
    report: &mut ValidationReport,
) {
    let env_type = req.task_env.env_type.trim();

    if env.lifecycle == dto::EnvLifecycle::Deprecated {
        report.push_warning(
            "task_env.env_type",
            match &env.superseded_by {
                Some(successor) => format!(
                    "'{env_type}' is deprecated; '{successor}' is the current name for this \
                     Task Environment"
                ),
                None => format!("'{env_type}' is deprecated"),
            },
        );
    }

    // A stack pinning `latest` inherits whatever `latest` resolves to later, so a
    // gate-blocked resolution today is informational. Pinning an exact version
    // that the gate barred is a different statement — the publisher chose it.
    if !env.latest_eligible {
        let msg = format!(
            "{env_type}@{} is barred from 'latest' by the rubric promotion gate; the stack \
             would run against a scoring version the Hub does not consider aligned",
            env.resolved_version
        );
        if req.task_env.version.trim() == "latest" {
            report.push_warning("task_env.version", msg);
        } else {
            report.push_error("task_env.version", msg);
        }
    }

    if let Some(dataset) = req.task_env.dataset.as_deref() {
        if let Some(keys) = &env.dataset_keys {
            if !keys.iter().any(|k| k == dataset) {
                report.push_error(
                    "task_env.dataset",
                    format!(
                        "'{dataset}' is not accepted by {env_type}@{}'s config_schema \
                         (accepts: {})",
                        env.resolved_version,
                        keys.join(", ")
                    ),
                );
            }
        }
    }

    let Some(scaffold) = scaffold else {
        return;
    };

    // The core pairing check. A scaffold declares which Task Environments it can
    // drive; honouring that declaration is the difference between a rejected
    // publish and a dispatch-time failure.
    if !scaffold.required_env_types.is_empty()
        && !scaffold
            .required_env_types
            .iter()
            .any(|t| t == env_type)
    {
        report.push_error(
            "agent_scaffold.package_id",
            format!(
                "scaffold '{}@{}' drives {} but this stack's Task Environment is '{env_type}'",
                req.agent_scaffold
                    .as_ref()
                    .map(|s| s.package_id.as_str())
                    .unwrap_or("?"),
                scaffold.resolved_version,
                scaffold.required_env_types.join(" / ")
            ),
        );
    }

    if let Some(expected) = req
        .agent_scaffold
        .as_ref()
        .and_then(|s| s.agent_kind.as_deref())
    {
        match scaffold.agent_kind.as_deref() {
            Some(actual) if actual == expected => {}
            Some(actual) => report.push_error(
                "agent_scaffold.agent_kind",
                format!("stack expects scaffold family '{expected}' but the package publishes '{actual}'"),
            ),
            None => report.push_warning(
                "agent_scaffold.agent_kind",
                format!(
                    "stack expects family '{expected}' but the package declares no \
                     agent_defaults.agent_kind, so the expectation cannot be verified"
                ),
            ),
        }
    }

    if let Some(consumer) = req
        .agent_scaffold
        .as_ref()
        .and_then(|s| s.consumer.as_deref())
    {
        let allowed = if scaffold.consumers.is_empty() {
            consumer == dto::CONSUMER_WORKER
        } else {
            scaffold.consumers.iter().any(|c| c == consumer)
        };
        if !allowed {
            let declared = if scaffold.consumers.is_empty() {
                "worker (implicit)".to_string()
            } else {
                scaffold.consumers.join(", ")
            };
            report.push_error(
                "agent_scaffold.consumer",
                format!(
                    "the Agent host would sync as '{consumer}' but the package is published \
                     for {declared}; republish with that consumer declared so both ends \
                     consume one digest"
                ),
            );
        }
    }

    // Gateway-bound environments in agent mode: the scaffold is on another host,
    // so without a gateway its commands execute locally and every task fails
    // while looking configured.
    if req.execution_mode == dto::ExecutionMode::Agent
        && GATEWAY_BOUND_ENV_TYPES.contains(&env_type)
        && !req.runtime_gateway.required
    {
        report.push_error(
            "runtime_gateway.required",
            format!(
                "'{env_type}' in agent mode runs the scaffold on a different host from the \
                 environment, so its terminal commands must be routed through the Worker \
                 Runtime Gateway; without it they execute on the Agent host and no task can \
                 pass"
            ),
        );
    }
}

/// Combined digest over the resolved component coordinates.
///
/// Deliberately derived from the resolved triples rather than from the stack
/// declaration: two runs of a stack pinning `latest` are the same run only if
/// `latest` meant the same thing, and this value says so.
pub fn stack_digest(components: &[dto::ResolvedComponent]) -> String {
    use sha2::{Digest, Sha256};
    let mut sorted: Vec<String> = components
        .iter()
        .map(|c| {
            format!(
                "{}\u{1f}{}\u{1f}{}\u{1f}{}",
                c.role,
                c.id,
                c.resolved,
                c.digest.as_deref().unwrap_or("")
            )
        })
        .collect();
    sorted.sort();
    let mut hasher = Sha256::new();
    for line in sorted {
        hasher.update(line.as_bytes());
        hasher.update(b"\n");
    }
    format!("sha256:{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent_req(env_type: &str, gateway: bool) -> dto::PublishStackRequest {
        dto::PublishStackRequest {
            version: "1.0.0".into(),
            publisher: None,
            description: None,
            changelog: None,
            execution_mode: dto::ExecutionMode::Agent,
            task_env: dto::TaskEnvRef {
                env_type: env_type.into(),
                version: "latest".into(),
                dataset: None,
            },
            agent_scaffold: Some(dto::AgentScaffoldRef {
                package_id: "uenv-agent-openhands".into(),
                version: "latest".into(),
                agent_kind: Some("openhands".into()),
                consumer: Some(dto::CONSUMER_OPENHANDS_AGENT.into()),
            }),
            runtime_gateway: dto::RuntimeGatewayReq {
                required: gateway,
                api: Some("runtime/v1".into()),
                api_key_required: true,
            },
            env_packages: vec!["swe-bench-verified@1.0.0".into()],
            required_worker_features: vec!["runtime_gateway".into()],
        }
    }

    fn env_facts() -> TaskEnvFacts {
        TaskEnvFacts {
            resolved_version: "0.1.0".into(),
            dataset_keys: None,
            latest_eligible: true,
            lifecycle: dto::EnvLifecycle::Active,
            superseded_by: None,
        }
    }

    fn scaffold_facts(envs: &[&str]) -> ScaffoldFacts {
        ScaffoldFacts {
            resolved_version: "1.0.1".into(),
            agent_kind: Some("openhands".into()),
            required_env_types: envs.iter().map(|s| s.to_string()).collect(),
            consumers: vec![dto::CONSUMER_OPENHANDS_AGENT.into()],
        }
    }

    #[test]
    fn a_well_formed_agent_stack_validates() {
        let mut report = ValidationReport::ok();
        validate(&agent_req("swe", true), &mut report);
        assert!(report.valid, "{:?}", report.issues);
    }

    #[test]
    fn agent_mode_without_a_scaffold_is_rejected() {
        let mut req = agent_req("swe", true);
        req.agent_scaffold = None;
        let mut report = ValidationReport::ok();
        validate(&req, &mut report);
        assert!(!report.valid);
        assert!(report
            .issues
            .iter()
            .any(|i| i.location == "agent_scaffold"));
    }

    #[test]
    fn native_mode_with_a_scaffold_is_rejected() {
        let mut req = agent_req("qa", true);
        req.execution_mode = dto::ExecutionMode::Native;
        let mut report = ValidationReport::ok();
        validate(&req, &mut report);
        assert!(!report.valid);
    }

    #[test]
    fn env_packages_must_be_pinned() {
        let mut req = agent_req("swe", true);
        req.env_packages = vec!["swe-bench-verified".into()];
        let mut report = ValidationReport::ok();
        validate(&req, &mut report);
        assert!(!report.valid);
        assert!(report.issues.iter().any(|i| i.location == "env_packages[0]"));
    }

    #[test]
    fn a_gateway_bound_env_in_agent_mode_requires_the_gateway() {
        let req = agent_req("swe", false);
        let mut report = ValidationReport::ok();
        cross_check(&req, &env_facts(), Some(&scaffold_facts(&["swe"])), &mut report);
        assert!(!report.valid);
        let issue = report
            .issues
            .iter()
            .find(|i| i.location == "runtime_gateway.required")
            .expect("gateway issue");
        assert!(issue.message.contains("Runtime Gateway"));
    }

    #[test]
    fn a_scaffold_that_drives_another_env_is_rejected() {
        let req = agent_req("code", true);
        let mut report = ValidationReport::ok();
        cross_check(&req, &env_facts(), Some(&scaffold_facts(&["swe"])), &mut report);
        assert!(!report.valid);
        assert!(report
            .issues
            .iter()
            .any(|i| i.location == "agent_scaffold.package_id"));
    }

    #[test]
    fn a_scaffold_family_mismatch_is_rejected() {
        let req = agent_req("swe", true);
        let mut facts = scaffold_facts(&["swe"]);
        facts.agent_kind = Some("toolenv".into());
        let mut report = ValidationReport::ok();
        cross_check(&req, &env_facts(), Some(&facts), &mut report);
        assert!(!report.valid);
        assert!(report
            .issues
            .iter()
            .any(|i| i.location == "agent_scaffold.agent_kind"));
    }

    #[test]
    fn a_consumer_the_package_does_not_publish_for_is_rejected() {
        let mut req = agent_req("swe", true);
        req.agent_scaffold.as_mut().unwrap().consumer =
            Some(dto::CONSUMER_TOOLENV_AGENT.into());
        let mut report = ValidationReport::ok();
        cross_check(&req, &env_facts(), Some(&scaffold_facts(&["swe"])), &mut report);
        assert!(!report.valid);
        assert!(report
            .issues
            .iter()
            .any(|i| i.location == "agent_scaffold.consumer"));
    }

    #[test]
    fn a_dataset_the_env_cannot_run_is_rejected() {
        let mut req = agent_req("code", true);
        req.task_env.dataset = Some("gsm8k".into());
        req.agent_scaffold.as_mut().unwrap().agent_kind = None;
        let mut facts = env_facts();
        facts.dataset_keys = Some(vec!["dscodebench".into()]);
        let mut report = ValidationReport::ok();
        cross_check(&req, &facts, Some(&scaffold_facts(&["code"])), &mut report);
        assert!(!report.valid);
        assert!(report.issues.iter().any(|i| i.location == "task_env.dataset"));
    }

    #[test]
    fn pinning_a_gate_blocked_exact_version_is_an_error_but_latest_is_a_warning() {
        let mut facts = env_facts();
        facts.latest_eligible = false;

        let mut req = agent_req("swe", true);
        req.task_env.version = "0.4.1".into();
        let mut report = ValidationReport::ok();
        cross_check(&req, &facts, Some(&scaffold_facts(&["swe"])), &mut report);
        assert!(!report.valid, "exact pin of a barred version must fail");

        let req = agent_req("swe", true); // version = "latest"
        let mut report = ValidationReport::ok();
        cross_check(&req, &facts, Some(&scaffold_facts(&["swe"])), &mut report);
        assert!(report.valid, "{:?}", report.issues);
        assert!(report
            .issues
            .iter()
            .any(|i| i.location == "task_env.version"));
    }

    #[test]
    fn a_deprecated_task_env_warns_and_names_its_successor() {
        let mut facts = env_facts();
        facts.lifecycle = dto::EnvLifecycle::Deprecated;
        facts.superseded_by = Some("qa".into());
        let mut req = agent_req("math", true);
        req.execution_mode = dto::ExecutionMode::Native;
        req.agent_scaffold = None;
        req.runtime_gateway.required = false;
        let mut report = ValidationReport::ok();
        cross_check(&req, &facts, None, &mut report);
        assert!(report.valid, "{:?}", report.issues);
        assert!(report
            .issues
            .iter()
            .any(|i| i.message.contains("qa")));
    }

    #[test]
    fn the_stack_digest_is_order_independent_but_version_sensitive() {
        let a = dto::ResolvedComponent {
            role: "task_env".into(),
            id: "swe".into(),
            requested: "latest".into(),
            resolved: "0.1.0".into(),
            digest: None,
            url: None,
        };
        let b = dto::ResolvedComponent {
            role: "agent_scaffold".into(),
            id: "uenv-agent-openhands".into(),
            requested: "latest".into(),
            resolved: "1.0.1".into(),
            digest: Some("sha256:aa".into()),
            url: None,
        };
        assert_eq!(
            stack_digest(&[a.clone(), b.clone()]),
            stack_digest(&[b.clone(), a.clone()])
        );

        let mut bumped = a.clone();
        bumped.resolved = "0.2.0".into();
        assert_ne!(stack_digest(&[a, b.clone()]), stack_digest(&[bumped, b]));
    }
}
