//! Pre-packaging conformance gate.
//!
//! Publishing or packaging an environment must be preceded by a **complete,
//! reproducible test** of its declaration. `uenv env validate` answers "is this
//! manifest well-formed"; this module answers the stricter question "is this
//! environment fit to be packaged for an air-gapped intranet", and emits a
//! machine-readable report that can be attached to the resulting EnvPackage as
//! evidence that the gate ran.
//!
//! Two deliberate differences from [`crate::domain::manifest`]:
//!
//! * **Zero egress is an error here, not advice.** A public-registry reference is
//!   acceptable while scaffolding, never in a package destined for an intranet.
//! * **Contract drift is checked against the implementation.** When the OpenEnv
//!   `models.py` is available, the declared `[interface.*]` properties are
//!   compared with the classes that actually serialise, so a manifest cannot
//!   silently diverge from the code.

use crate::domain::manifest;
use crate::domain::openenv::{self, Kind};
use crate::domain::rubric;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use uenv_hub_types::{PublishVersionRequest, Severity};

/// Outcome of a single check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CheckStatus {
    Pass,
    /// Non-blocking finding: packaging may proceed, `--strict` refuses.
    Warn,
    /// Blocking finding: the environment must not be packaged.
    Fail,
    /// Not applicable / no evidence supplied.
    Skip,
}

/// One auditable check. `id` is stable so reports can be compared over time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Check {
    pub id: String,
    pub title: String,
    pub status: CheckStatus,
    pub detail: String,
}

impl Check {
    fn new(
        id: &str,
        title: &str,
        status: CheckStatus,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            id: id.to_string(),
            title: title.to_string(),
            status,
            detail: detail.into(),
        }
    }
}

/// The full gate result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConformanceReport {
    pub env_type: String,
    pub version: String,
    pub gate_version: String,
    pub checks: Vec<Check>,
    /// No `Fail` check.
    pub passed: bool,
    /// No `Fail` and no `Warn` — required by `uenv env test --strict`.
    pub strict_passed: bool,
    pub failed: usize,
    pub warned: usize,
}

/// Gate revision, recorded in every report, so a stored report can be read
/// against the rule set that produced it. `/2` added C12 (rubric contract); `/3`
/// added C13 (the gold-standard rule package is distributable, not just named).
/// C01–C11 semantics are unchanged across both bumps.
pub const GATE_VERSION: &str = "uenv-conformance/3";

/// Evidence that the offline (air-gapped) preparation actually happened.
/// Supplied by the CLI after inspecting the project directory; `None` fields
/// mean "no evidence", which is reported as such rather than assumed.
#[derive(Debug, Clone, Default)]
pub struct OfflineEvidence {
    /// Number of wheels found in the offline wheelhouse.
    pub wheel_count: Option<usize>,
    /// Number of pre-compiled `.pyc` files found.
    pub pyc_count: Option<usize>,
    /// A `docker save` archive staged for the runtime image.
    pub image_tar_present: Option<bool>,
    /// Python sources counted in the project (denominator for precompilation).
    pub py_source_count: Option<usize>,
    /// From `offline/precompile.json`: the wheelhouse contains platform-specific
    /// wheels that do not match the declared target platform. Such a wheelhouse
    /// installs on the preparation host but fails on the air-gapped worker, where
    /// there is no PyPI to fall back to.
    pub platform_mismatch: Option<bool>,
    /// Target platform recorded by the precompile run, for the report.
    pub target_platform: Option<String>,
}

/// Inputs to the gate beyond the publish request itself.
#[derive(Debug, Clone, Default)]
pub struct GateOptions<'a> {
    /// Text of `models.py`, enabling the contract-drift check.
    pub models_src: Option<&'a str>,
    pub offline: OfflineEvidence,
    /// Registry hosts that count as intranet. When non-empty, C06 switches from
    /// "reject known public registries" to "reject everything not listed", which
    /// is the only decidable form of the zero-egress rule.
    pub intranet_registries: Vec<String>,
    /// Thresholds for C12. Defaults match the aligner's own defaults so the gate
    /// does not re-judge a corpus run that already passed locally.
    pub rubric_gate: rubric::GateOptions,
}

fn schema_property_names(schema: Option<&serde_json::Value>) -> BTreeSet<String> {
    schema
        .and_then(|s| s.get("properties"))
        .and_then(|p| p.as_object())
        .map(|o| o.keys().cloned().collect())
        .unwrap_or_default()
}

/// Run the gate.
pub fn run(env_type: &str, req: &PublishVersionRequest, opts: &GateOptions<'_>) -> ConformanceReport {
    let mut checks: Vec<Check> = Vec::new();

    // ---- C01: structural validity (shared with validate / server publish) ----
    let report = manifest::validate_manifest(env_type, req);
    let errors: Vec<String> = report
        .issues
        .iter()
        .filter(|i| i.severity == Severity::Error)
        .map(|i| format!("{}: {}", i.location, i.message))
        .collect();
    checks.push(if errors.is_empty() {
        Check::new(
            "C01",
            "manifest structural validity",
            CheckStatus::Pass,
            "no structural errors (same rules as the server publish path)",
        )
    } else {
        Check::new(
            "C01",
            "manifest structural validity",
            CheckStatus::Fail,
            errors.join("; "),
        )
    });

    // ---- C02: OpenEnv contract completeness ----
    let missing: Vec<&str> = [
        ("action", req.interface.action.is_some()),
        ("observation", req.interface.observation.is_some()),
        ("state", req.interface.state.is_some()),
    ]
    .into_iter()
    .filter(|(_, present)| !present)
    .map(|(k, _)| k)
    .collect();
    checks.push(if missing.is_empty() {
        Check::new(
            "C02",
            "OpenEnv contract completeness (action/observation/state)",
            CheckStatus::Pass,
            "all three JSON Schemas declared",
        )
    } else {
        Check::new(
            "C02",
            "OpenEnv contract completeness (action/observation/state)",
            CheckStatus::Fail,
            format!("missing interface schema(s): {}", missing.join(", ")),
        )
    });

    // ---- C03: each declared schema compiles as a JSON Schema ----
    let iface_report = manifest::validate_interface_only(&req.interface);
    checks.push(if iface_report.valid {
        Check::new(
            "C03",
            "interface schemas compile",
            CheckStatus::Pass,
            "action/observation/state are valid JSON Schema documents",
        )
    } else {
        Check::new(
            "C03",
            "interface schemas compile",
            CheckStatus::Fail,
            iface_report
                .issues
                .iter()
                .map(|i| format!("{}: {}", i.location, i.message))
                .collect::<Vec<_>>()
                .join("; "),
        )
    });

    // ---- C04: declared contract vs. implementation (drift) ----
    checks.push(match opts.models_src {
        None => Check::new(
            "C04",
            "contract matches implementation (models.py)",
            CheckStatus::Skip,
            "models.py not supplied; drift cannot be checked",
        ),
        Some(src) => {
            let classes = openenv::parse_python_models(src);
            if classes.is_empty() {
                Check::new(
                    "C04",
                    "contract matches implementation (models.py)",
                    CheckStatus::Warn,
                    "no Action/Observation/State classes found in models.py",
                )
            } else {
                let mut drift: Vec<String> = Vec::new();
                for (label, declared, kind) in [
                    ("action", req.interface.action.as_ref(), Kind::Action),
                    (
                        "observation",
                        req.interface.observation.as_ref(),
                        Kind::Observation,
                    ),
                    ("state", req.interface.state.as_ref(), Kind::State),
                ] {
                    let Some(impl_schema) = openenv::schema_for(&classes, None, kind) else {
                        continue;
                    };
                    let from_impl = schema_property_names(Some(&impl_schema));
                    let from_manifest = schema_property_names(declared);
                    let only_impl: Vec<&String> = from_impl.difference(&from_manifest).collect();
                    let only_manifest: Vec<&String> =
                        from_manifest.difference(&from_impl).collect();
                    if !only_impl.is_empty() {
                        drift.push(format!(
                            "{label}: implemented but undeclared {only_impl:?}"
                        ));
                    }
                    if !only_manifest.is_empty() {
                        drift.push(format!(
                            "{label}: declared but not implemented {only_manifest:?}"
                        ));
                    }
                }
                if drift.is_empty() {
                    Check::new(
                        "C04",
                        "contract matches implementation (models.py)",
                        CheckStatus::Pass,
                        "declared properties match the pydantic models field-for-field",
                    )
                } else {
                    Check::new(
                        "C04",
                        "contract matches implementation (models.py)",
                        CheckStatus::Fail,
                        drift.join("; "),
                    )
                }
            }
        }
    });

    // ---- C05: examples exist and satisfy the action schema ----
    let example_errors: Vec<String> = report
        .issues
        .iter()
        .filter(|i| i.location.starts_with("examples[") && i.severity == Severity::Error)
        .map(|i| format!("{}: {}", i.location, i.message))
        .collect();
    checks.push(if req.examples.is_empty() {
        Check::new(
            "C05",
            "examples present and conform to the action schema",
            CheckStatus::Warn,
            "no examples/*.json supplied; the contract is undemonstrated",
        )
    } else if example_errors.is_empty() {
        Check::new(
            "C05",
            "examples present and conform to the action schema",
            CheckStatus::Pass,
            format!("{} example(s) validated against interface.action", req.examples.len()),
        )
    } else {
        Check::new(
            "C05",
            "examples present and conform to the action schema",
            CheckStatus::Fail,
            example_errors.join("; "),
        )
    });

    // ---- C06: zero egress (blocking here) ----
    //
    // Two modes, because a blacklist of public registries can never be complete
    // — new Docker Hub mirrors appear all the time, and each one is as external
    // as the upstream it proxies:
    //
    // * default: reject *known* public registries (decidable, no configuration);
    // * `--intranet-registry <host>` given: reject every host that is **not**
    //   allowed (decidable and complete, at the cost of declaring the intranet).
    let refs: Vec<(&str, &str)> = [
        req.image.as_ref().map(|i| ("image.url", i.url.as_str())),
        req.image
            .as_ref()
            .and_then(|i| i.base_image_ref.as_deref())
            .map(|b| ("image.base_image_ref", b)),
        req.base_image.as_deref().map(|b| ("version.base_image", b)),
    ]
    .into_iter()
    .flatten()
    .collect();

    let allowlist: Vec<String> = opts
        .intranet_registries
        .iter()
        .map(|h| h.trim().to_ascii_lowercase())
        .filter(|h| !h.is_empty())
        .collect();

    let mut offenders: Vec<String> = Vec::new();
    let mut hostless: Vec<String> = Vec::new();
    for (field, value) in &refs {
        if allowlist.is_empty() {
            if let Some(reg) = manifest::public_registry_of(value) {
                offenders.push(format!("{field} -> {reg}"));
            }
        } else if manifest::resolves_to_docker_hub(value) {
            // No registry host under the reference grammar (`echo-env:1.0.0`,
            // `swebench/x:latest`). Ambiguous by construction: the worker's local
            // image store if something loaded it there, `docker.io/…` otherwise.
            // The manifest alone cannot say which, so it is reported rather than
            // assumed either way.
            hostless.push(format!("{field} -> {value}"));
        } else {
            let host = value
                .trim()
                .split_once('/')
                .map(|(h, _)| h.to_ascii_lowercase())
                .unwrap_or_default();
            if !allowlist.iter().any(|a| a == &host) {
                offenders.push(format!(
                    "{field} -> {host} (not in the --intranet-registry allowlist)"
                ));
            }
        }
    }
    let mode = if allowlist.is_empty() {
        "known-public denylist".to_string()
    } else {
        format!("intranet allowlist [{}]", allowlist.join(", "))
    };
    checks.push(if !offenders.is_empty() {
        Check::new(
            "C06",
            "zero egress: no public container registry references",
            CheckStatus::Fail,
            format!(
                "packaging for an intranet must not reference external registries ({mode}): {}",
                offenders.join(", ")
            ),
        )
    } else if !hostless.is_empty() {
        Check::new(
            "C06",
            "zero egress: no public container registry references",
            CheckStatus::Warn,
            format!(
                "{mode}: reference(s) without a registry host ({}) are only intranet-safe if the \
                 image is already in the worker's local store — ship it as a Hub-hosted tar \
                 (`uenv env publish-image` + `uenv env sync --docker-load`); a container engine \
                 would otherwise resolve them against docker.io",
                hostless.join(", ")
            ),
        )
    } else {
        Check::new(
            "C06",
            "zero egress: no public container registry references",
            CheckStatus::Pass,
            format!("{mode}: every image reference is intranet-reachable or Hub-hosted"),
        )
    });

    // ---- C07: runtime image declared and digest-pinned ----
    checks.push(match &req.image {
        None => Check::new(
            "C07",
            "runtime image declared and digest-pinned",
            CheckStatus::Warn,
            "no [image] declared; the worker can only launch via entrypoint",
        ),
        Some(image) => match &image.digest {
            Some(d) if d.starts_with("sha256:") && d.len() >= "sha256:".len() + 8 => Check::new(
                "C07",
                "runtime image declared and digest-pinned",
                CheckStatus::Pass,
                format!("{} pinned by {}", image.url, d),
            ),
            _ => Check::new(
                "C07",
                "runtime image declared and digest-pinned",
                CheckStatus::Warn,
                format!("{} has no sha256 digest; tampering cannot be detected", image.url),
            ),
        },
    });

    // ---- C08: config schema / default config consistency ----
    let cfg_errors: Vec<String> = report
        .issues
        .iter()
        .filter(|i| {
            i.severity == Severity::Error
                && (i.location == "config_schema" || i.location == "default_config")
        })
        .map(|i| format!("{}: {}", i.location, i.message))
        .collect();
    checks.push(if !cfg_errors.is_empty() {
        Check::new(
            "C08",
            "config_schema / default_config consistency",
            CheckStatus::Fail,
            cfg_errors.join("; "),
        )
    } else if req.config_schema.is_some() {
        Check::new(
            "C08",
            "config_schema / default_config consistency",
            CheckStatus::Pass,
            "config_schema is a valid schema and default_config satisfies it",
        )
    } else {
        Check::new(
            "C08",
            "config_schema / default_config consistency",
            CheckStatus::Skip,
            "no config_schema declared",
        )
    });

    // ---- C09: launchable ----
    let has_entrypoint = req
        .entrypoint
        .as_deref()
        .map(|e| !e.trim().is_empty())
        .unwrap_or(false);
    checks.push(if has_entrypoint || req.image.is_some() {
        Check::new(
            "C09",
            "environment is launchable (entrypoint or image)",
            CheckStatus::Pass,
            if has_entrypoint {
                "version.entrypoint declared"
            } else {
                "[image] declared; launch via image CMD"
            },
        )
    } else {
        Check::new(
            "C09",
            "environment is launchable (entrypoint or image)",
            CheckStatus::Fail,
            "neither version.entrypoint nor [image] declared",
        )
    });

    // ---- C10: health check path ----
    checks.push(match req.health_check_path.as_deref() {
        Some(p) if p.starts_with('/') => Check::new(
            "C10",
            "health check path declared",
            CheckStatus::Pass,
            format!("health_check_path={p}"),
        ),
        Some(p) => Check::new(
            "C10",
            "health check path declared",
            CheckStatus::Warn,
            format!("health_check_path '{p}' should start with '/'"),
        ),
        None => Check::new(
            "C10",
            "health check path declared",
            CheckStatus::Warn,
            "no health_check_path; readiness cannot be probed (OpenEnv serves /health)",
        ),
    });

    // ---- C11: offline precompilation evidence ----
    checks.push(offline_check(&opts.offline, req));

    // ---- C12: rubric contract & gold-standard alignment ----
    checks.push(rubric_check(req, &opts.rubric_gate));

    // ---- C13: the gold-standard rule package itself is distributable ----
    checks.push(rubric_scorer_check(req));

    let failed = checks.iter().filter(|c| c.status == CheckStatus::Fail).count();
    let warned = checks.iter().filter(|c| c.status == CheckStatus::Warn).count();
    ConformanceReport {
        env_type: env_type.to_string(),
        version: req.version.clone(),
        gate_version: GATE_VERSION.to_string(),
        checks,
        passed: failed == 0,
        strict_passed: failed == 0 && warned == 0,
        failed,
        warned,
    }
}

fn offline_check(ev: &OfflineEvidence, req: &PublishVersionRequest) -> Check {
    let declares_deps = req
        .dependencies
        .as_ref()
        .map(|d| d.requirements_path.is_some() || !d.requires.is_empty())
        .unwrap_or(false);
    match (ev.wheel_count, ev.pyc_count) {
        (Some(wheels), Some(pyc)) => {
            let mut detail = format!("wheelhouse={wheels} wheel(s), precompiled={pyc} .pyc");
            if let Some(total) = ev.py_source_count {
                detail.push_str(&format!(" / {total} .py source(s)"));
            }
            if let Some(tar) = ev.image_tar_present {
                detail.push_str(&format!(", image_tar={}", if tar { "staged" } else { "absent" }));
            }
            if let Some(platform) = &ev.target_platform {
                if !platform.is_empty() {
                    detail.push_str(&format!(", target_platform={platform}"));
                }
            }
            if ev.platform_mismatch == Some(true) {
                return Check::new(
                    "C11",
                    "offline precompilation prepared (wheels + bytecode)",
                    CheckStatus::Fail,
                    format!(
                        "{detail}; the wheelhouse holds platform-specific wheels that do not match the \
                         target platform — it would fail to install on the air-gapped worker. Re-run \
                         openenv-offline-precompile.sh with --platform/--python-version for the target."
                    ),
                );
            }
            if wheels == 0 && declares_deps {
                Check::new(
                    "C11",
                    "offline precompilation prepared (wheels + bytecode)",
                    CheckStatus::Fail,
                    format!("{detail}; dependencies are declared but the offline wheelhouse is empty — an air-gapped install would need PyPI"),
                )
            } else if pyc == 0 {
                Check::new(
                    "C11",
                    "offline precompilation prepared (wheels + bytecode)",
                    CheckStatus::Warn,
                    format!("{detail}; no precompiled bytecode (first-run import will compile in-container)"),
                )
            } else {
                Check::new(
                    "C11",
                    "offline precompilation prepared (wheels + bytecode)",
                    CheckStatus::Pass,
                    detail,
                )
            }
        }
        _ => Check::new(
            "C11",
            "offline precompilation prepared (wheels + bytecode)",
            CheckStatus::Skip,
            "no offline evidence supplied (run with --project pointing at the prepared env)",
        ),
    }
}

/// C12 — the rubric contract, for environments that reward by rule.
///
/// Skipped when no rubric is declared: an environment that rewards by executing
/// tests (`code`, `swe`) has no rule to align, and forcing a rubric on it would
/// only produce a meaningless one.
///
/// When a rubric *is* declared, over-credit is a `Fail` rather than a `Warn`.
/// Over-credit means the shipped scorer pays out where the reference scorer does
/// not, which is precisely the surface a policy learns to exploit; packaging such
/// a version as the default would train against a reward the benchmark does not
/// endorse.
const C12_TITLE: &str = "rubric contract & gold-standard alignment";

fn rubric_check(req: &PublishVersionRequest, gate: &rubric::GateOptions) -> Check {
    let Some(spec) = &req.rubric else {
        return Check::new(
            "C12",
            C12_TITLE,
            CheckStatus::Skip,
            "no [rubric] declared; applies to rule-scored environments (e.g. qa) only",
        );
    };

    // Structural coherence first, cross-checked against the dataset routing.
    let dataset_keys = req
        .config_schema
        .as_ref()
        .and_then(rubric::dataset_keys_from_config_schema);
    let mut report = uenv_hub_types::ValidationReport::ok();
    rubric::validate(spec, dataset_keys.as_deref(), &mut report);
    let errors: Vec<String> = report
        .issues
        .iter()
        .filter(|i| i.severity == Severity::Error)
        .map(|i| format!("{}: {}", i.location, i.message))
        .collect();
    if !errors.is_empty() {
        return Check::new(
            "C12",
            C12_TITLE,
            CheckStatus::Fail,
            format!("invalid rubric contract: {}", errors.join("; ")),
        );
    }

    let outcome = rubric::gate(Some(spec), gate);
    if !outcome.eligible {
        return Check::new(
            "C12",
            C12_TITLE,
            CheckStatus::Fail,
            format!(
                "gold-standard alignment insufficient: {}. Re-run \
                 verify_qa_rubric_alignment.py and fix the scorer before packaging.",
                outcome.notes.join("; ")
            ),
        );
    }

    let alignment = spec.alignment.as_ref();
    let metrics = alignment.and_then(|a| a.metrics.as_ref());
    let mut detail = match metrics {
        Some(m) => format!(
            "agreement={:.4}, over_credit={}, under_credit={}",
            m.agreement_rate, m.over_credit_count, m.under_credit_count
        ),
        None => "no metrics".to_string(),
    };
    if let Some(scorer) = &spec.production_scorer {
        detail.push_str(&format!(", scorer={scorer}"));
    }

    // Traceability: metrics without the corpus/report digests cannot be re-derived
    // later, so the claim is unverifiable even though it is not wrong.
    let missing_evidence = alignment
        .map(|a| a.corpus_digest.is_none() || a.report_digest.is_none())
        .unwrap_or(true);
    if missing_evidence {
        return Check::new(
            "C12",
            C12_TITLE,
            CheckStatus::Warn,
            format!(
                "{detail}; corpus/report digest missing — the alignment claim is not \
                 reproducible. Attach evidence with `uenv env rubric import --metrics … \
                 --corpus …`."
            ),
        );
    }

    Check::new("C12", C12_TITLE, CheckStatus::Pass, detail)
}

/// C13 — is the gold standard itself distributable, or only described?
///
/// C12 checks the *measurement*: how closely production agreed with a reference,
/// on which corpus. It cannot check the *reference*, because `backend:
/// "verifiers+math_verify"` names a library rather than a rule package, and for a
/// verification environment the rules are what determine the score. Swapping
/// GSM8K's `####` extraction for a boxed-only parser leaves the backend string
/// untouched and turns every GSM8K case from correct to zero.
///
/// So this check asks a separate question: can a consumer obtain the exact rules
/// the recorded agreement was measured with? A declared-but-unfetchable gold
/// standard is a `Warn` rather than a `Fail` — the alignment number is still real,
/// it just cannot be independently re-derived — while a malformed coordinate is a
/// `Fail`, because a digest that cannot be parsed will never verify.
const C13_TITLE: &str = "rubric gold-standard rule package is Hub-distributable";

fn rubric_scorer_check(req: &PublishVersionRequest) -> Check {
    let Some(spec) = &req.rubric else {
        return Check::new(
            "C13",
            C13_TITLE,
            CheckStatus::Skip,
            "no [rubric] declared; applies to rule-scored environments (e.g. qa) only",
        );
    };

    let Some(scorer) = &spec.reference_scorer else {
        return Check::new(
            "C13",
            C13_TITLE,
            CheckStatus::Warn,
            "no [rubric.reference_scorer] declared: the gold standard is named by library \
             only, so a consumer cannot fetch the extraction rules the agreement was \
             measured against. Publish them with `uenv env rubric publish <pkg> --scorer \
             qa_rubric.py` and reference them via `uenv env rubric import --scorer-ref`.",
        );
    };

    let mut faults: Vec<String> = Vec::new();
    if !scorer.package_ref.contains('@') {
        faults.push(format!(
            "package_ref '{}' is not 'package_id@version'",
            scorer.package_ref
        ));
    }
    if scorer.artifact.trim().is_empty() {
        faults.push("artifact name is empty".to_string());
    }
    if !is_sha256_digest(&scorer.digest) {
        faults.push(format!(
            "digest '{}' is not a 'sha256:<hex>' value, so the bytes can never be verified",
            scorer.digest
        ));
    }
    if !faults.is_empty() {
        return Check::new(
            "C13",
            C13_TITLE,
            CheckStatus::Fail,
            format!("unusable reference_scorer coordinate: {}", faults.join("; ")),
        );
    }

    let mut detail = format!(
        "{} :: {} pinned by {}",
        scorer.package_ref, scorer.artifact, scorer.digest
    );
    if let Some(ep) = &scorer.entrypoint {
        detail.push_str(&format!(", entrypoint={ep}"));
    }
    if !scorer.rubric_classes.is_empty() {
        detail.push_str(&format!(
            ", verifiers classes=[{}]",
            scorer.rubric_classes.join(", ")
        ));
    }

    // Executable, not just downloadable: without an entrypoint the consumer has
    // the bytes and still has to guess how to invoke them.
    if scorer.entrypoint.is_none() {
        return Check::new(
            "C13",
            C13_TITLE,
            CheckStatus::Warn,
            format!("{detail}; no entrypoint declared, so the rules can be read but not run"),
        );
    }
    if scorer.requires.is_empty() {
        return Check::new(
            "C13",
            C13_TITLE,
            CheckStatus::Warn,
            format!(
                "{detail}; no `requires` declared, so an air-gapped consumer cannot tell \
                 which wheels it must have vendored to execute the rules"
            ),
        );
    }

    Check::new("C13", C13_TITLE, CheckStatus::Pass, detail)
}

fn is_sha256_digest(digest: &str) -> bool {
    digest
        .strip_prefix("sha256:")
        .map(|hex| hex.len() == 64 && hex.chars().all(|c| c.is_ascii_hexdigit()))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use uenv_hub_types::{Example, ImageSpec, InterfaceSchema, ResourceSpec};

    const MODELS: &str = r#"
from openenv.core.env_server.interfaces import Action, Observation, State

class CodeAction(Action):
    code: str

class CodeObservation(Observation):
    stdout: str = ""

class CodeState(State):
    last_exit_code: int = 0
"#;

    fn req_from_models() -> PublishVersionRequest {
        let classes = openenv::parse_python_models(MODELS);
        PublishVersionRequest {
            version: "0.1.0".into(),
            changelog: None,
            image: Some(ImageSpec {
                url: "registry.uenv.internal/openenv/coding-env:0.1.0".into(),
                digest: Some(format!("sha256:{}", "ab".repeat(32))),
                size_bytes: None,
                arch: Some("amd64".into()),
                base_image_ref: None,
            }),
            base_image: None,
            health_check_path: Some("/health".into()),
            entrypoint: Some("uvicorn server.app:app".into()),
            supported_backends: vec!["docker".into()],
            config_schema: None,
            default_config: None,
            resources: ResourceSpec::default(),
            interface: InterfaceSchema {
                action: openenv::schema_for(&classes, None, Kind::Action),
                observation: openenv::schema_for(&classes, None, Kind::Observation),
                state: openenv::schema_for(&classes, None, Kind::State),
            },
            examples: vec![Example {
                title: Some("demo".into()),
                request: json!({"actions": [{"code": "print(1)"}]}),
            }],
            dependencies: None,
            min_uenv_version: None,
            rubric: None,
        }
    }

    fn status_of<'a>(r: &'a ConformanceReport, id: &str) -> &'a Check {
        r.checks
            .iter()
            .find(|c| c.id.as_str() == id)
            .expect("check present")
    }

    #[test]
    fn aligned_environment_passes_gate() {
        let req = req_from_models();
        let opts = GateOptions {
            models_src: Some(MODELS),
            intranet_registries: Vec::new(),
            rubric_gate: Default::default(),
            offline: OfflineEvidence {
                wheel_count: Some(3),
                pyc_count: Some(12),
                py_source_count: Some(9),
                image_tar_present: Some(true),
                platform_mismatch: Some(false),
                target_platform: Some("manylinux2014_x86_64".into()),
            },
        };
        let r = run("coding-env", &req, &opts);
        assert!(r.passed, "{:#?}", r.checks);
        assert!(r.strict_passed, "{:#?}", r.checks);
        assert_eq!(status_of(&r, "C04").status, CheckStatus::Pass);
        assert_eq!(status_of(&r, "C06").status, CheckStatus::Pass);
        assert_eq!(status_of(&r, "C11").status, CheckStatus::Pass);
    }

    #[test]
    fn public_registry_blocks_packaging() {
        let mut req = req_from_models();
        req.image.as_mut().unwrap().url = "docker.io/library/python:3.11-slim".into();
        let r = run("coding-env", &req, &GateOptions::default());
        assert!(!r.passed, "public registry must block the gate");
        assert_eq!(status_of(&r, "C06").status, CheckStatus::Fail);
        assert!(status_of(&r, "C06").detail.contains("docker.io"));
    }

    #[test]
    fn public_hub_mirror_blocks_packaging_too() {
        // A mirror host is not an intranet registry, even though it is neither
        // docker.io nor a well-known cloud registry.
        let mut req = req_from_models();
        req.image.as_mut().unwrap().url =
            "dockerproxy.net/swebench/sweb.eval.x86_64.sympy-20916:latest".into();
        let r = run("coding-env", &req, &GateOptions::default());
        assert_eq!(status_of(&r, "C06").status, CheckStatus::Fail);
        assert!(status_of(&r, "C06").detail.contains("dockerproxy.net"));
    }

    #[test]
    fn intranet_allowlist_rejects_unknown_hosts_and_accepts_declared_ones() {
        let mut req = req_from_models();
        // A host that no denylist would ever contain, but is still not ours.
        req.image.as_mut().unwrap().url = "registry.example-vendor.cn/envs/coding-env:1.0.0".into();
        let denylist_only = run("coding-env", &req, &GateOptions::default());
        assert_eq!(
            status_of(&denylist_only, "C06").status,
            CheckStatus::Pass,
            "a denylist cannot know this host — which is exactly its limitation"
        );

        let opts = GateOptions {
            intranet_registries: vec!["registry.uenv.internal".into(), "192.168.0.133:5000".into()],
            ..Default::default()
        };
        let strict = run("coding-env", &req, &opts);
        let c06 = status_of(&strict, "C06");
        assert_eq!(c06.status, CheckStatus::Fail, "{}", c06.detail);
        assert!(c06.detail.contains("allowlist"), "{}", c06.detail);

        // A declared intranet host passes.
        req.image.as_mut().unwrap().url = "192.168.0.133:5000/envs/coding-env:1.0.0".into();
        assert_eq!(
            status_of(&run("coding-env", &req, &opts), "C06").status,
            CheckStatus::Pass
        );

        // A bare name may be the worker's local image store (what `docker load`
        // from a Hub-hosted tar produces) or docker.io. The manifest cannot say
        // which, so it is a warning: allowed, but only under `--strict` review.
        req.image.as_mut().unwrap().url = "coding-env:1.0.0".into();
        let hostless = run("coding-env", &req, &opts);
        let c06 = status_of(&hostless, "C06");
        assert_eq!(c06.status, CheckStatus::Warn, "{}", c06.detail);
        assert!(c06.detail.contains("publish-image"), "{}", c06.detail);

        // `user/name:tag` has no registry host either — it is Docker Hub under
        // the reference grammar, so it must be classified like the bare form and
        // not mistaken for a host called `swebench`.
        req.image.as_mut().unwrap().url = "swebench/sweb.eval.x86_64.sympy-20916:latest".into();
        let namespaced = run("coding-env", &req, &opts);
        let c06 = status_of(&namespaced, "C06");
        assert_eq!(c06.status, CheckStatus::Warn, "{}", c06.detail);
        assert!(
            !c06.detail.contains("not in the --intranet-registry allowlist"),
            "must not report `swebench` as a registry host: {}",
            c06.detail
        );
    }

    #[test]
    fn contract_drift_is_detected() {
        let mut req = req_from_models();
        // Implementation has `code`; declare something else → drift both ways.
        req.interface.action = Some(json!({
            "type": "object",
            "properties": {"snippet": {"type": "string"}},
            "required": ["snippet"]
        }));
        let opts = GateOptions {
            models_src: Some(MODELS),
            ..Default::default()
        };
        let r = run("coding-env", &req, &opts);
        let c04 = status_of(&r, "C04");
        assert_eq!(c04.status, CheckStatus::Fail, "{}", c04.detail);
        assert!(c04.detail.contains("code"), "{}", c04.detail);
        assert!(c04.detail.contains("snippet"), "{}", c04.detail);
    }

    #[test]
    fn missing_contract_side_blocks_packaging() {
        let mut req = req_from_models();
        req.interface.state = None;
        let r = run("coding-env", &req, &GateOptions::default());
        assert!(!r.passed);
        assert_eq!(status_of(&r, "C02").status, CheckStatus::Fail);
        assert!(status_of(&r, "C02").detail.contains("state"));
    }

    #[test]
    fn empty_wheelhouse_with_declared_deps_fails() {
        let mut req = req_from_models();
        req.dependencies = Some(uenv_hub_types::Dependencies {
            requirements_path: Some("requirements.txt".into()),
            install_script: None,
            requires: vec![],
        });
        let opts = GateOptions {
            models_src: Some(MODELS),
            intranet_registries: Vec::new(),
            rubric_gate: Default::default(),
            offline: OfflineEvidence {
                wheel_count: Some(0),
                pyc_count: Some(5),
                py_source_count: Some(5),
                image_tar_present: Some(false),
                ..Default::default()
            },
        };
        let r = run("coding-env", &req, &opts);
        assert!(!r.passed);
        assert_eq!(status_of(&r, "C11").status, CheckStatus::Fail);
    }

    /// Regression for a trap observed on the real hosts: wheels vendored on macOS
    /// install locally but fail on a Linux worker (`No matching distribution
    /// found for pydantic-core`), where there is no PyPI to recover from.
    #[test]
    fn non_portable_wheelhouse_blocks_packaging() {
        let req = req_from_models();
        let opts = GateOptions {
            models_src: Some(MODELS),
            intranet_registries: Vec::new(),
            rubric_gate: Default::default(),
            offline: OfflineEvidence {
                wheel_count: Some(4),
                pyc_count: Some(3),
                py_source_count: Some(3),
                image_tar_present: Some(false),
                platform_mismatch: Some(true),
                target_platform: Some("manylinux2014_x86_64".into()),
            },
        };
        let r = run("coding-env", &req, &opts);
        assert!(!r.passed, "a non-portable wheelhouse must block packaging");
        let c11 = status_of(&r, "C11");
        assert_eq!(c11.status, CheckStatus::Fail);
        assert!(c11.detail.contains("target platform"), "{}", c11.detail);
    }

    #[test]
    fn missing_examples_warns_but_does_not_block() {
        let mut req = req_from_models();
        req.examples.clear();
        let opts = GateOptions {
            models_src: Some(MODELS),
            ..Default::default()
        };
        let r = run("coding-env", &req, &opts);
        assert!(r.passed, "{:#?}", r.checks);
        assert!(!r.strict_passed, "strict mode must reject undemonstrated contracts");
        assert_eq!(status_of(&r, "C05").status, CheckStatus::Warn);
    }

    #[test]
    fn report_serializes_as_evidence() {
        let req = req_from_models();
        let r = run("coding-env", &req, &GateOptions::default());
        let json = serde_json::to_string_pretty(&r).unwrap();
        assert!(json.contains("\"gate_version\""));
        assert!(json.contains("\"C06\""));
        let back: ConformanceReport = serde_json::from_str(&json).unwrap();
        assert_eq!(back.env_type, "coding-env");
    }

    fn rubric_spec(over: i64, rate: f64, with_digests: bool) -> uenv_hub_types::RubricSpec {
        let d = |c: char| format!("sha256:{}", String::from(c).repeat(64));
        uenv_hub_types::RubricSpec {
            schema_version: "1".into(),
            backend: Some("verifiers+math_verify".into()),
            production_scorer: Some("uenv-math-plugin/score_action".into()),
            reference_scorer: with_digests.then(|| uenv_hub_types::RubricScorerRef {
                package_ref: "qa-rubric-scorer@0.1.0".into(),
                artifact: "qa_rubric.py".into(),
                digest: d('c'),
                entrypoint: Some("qa_rubric:score".into()),
                rubric_classes: vec!["Rubric".into(), "MathRubric".into()],
                requires: vec!["verifiers".into(), "math-verify".into()],
            }),
            alignment: Some(uenv_hub_types::RubricAlignment {
                corpus_id: Some("qa_rubric_corpus@2026-07-25".into()),
                corpus_digest: with_digests.then(|| d('a')),
                report_digest: with_digests.then(|| d('b')),
                package_ref: None,
                metrics: Some(uenv_hub_types::RubricMetrics {
                    total: None,
                    agreed: None,
                    agreement_rate: rate,
                    over_credit_count: over,
                    under_credit_count: 2,
                    verifiers_version: None,
                    math_verify_version: None,
                }),
            }),
            datasets: Default::default(),
            known_gaps: vec![],
        }
    }

    /// An execution-scored environment has no rule to align, so C12 must not
    /// invent a requirement for it.
    #[test]
    fn c12_is_skipped_without_a_rubric() {
        let req = req_from_models();
        let r = run("coding-env", &req, &GateOptions::default());
        assert_eq!(status_of(&r, "C12").status, CheckStatus::Skip);
        assert!(r.passed);
    }

    #[test]
    fn c12_passes_for_an_aligned_rubric_with_evidence() {
        let mut req = req_from_models();
        req.rubric = Some(rubric_spec(0, 0.9655, true));
        let r = run("qa", &req, &GateOptions::default());
        let c12 = status_of(&r, "C12");
        assert_eq!(c12.status, CheckStatus::Pass, "{}", c12.detail);
        assert!(c12.detail.contains("over_credit=0"), "{}", c12.detail);
    }

    /// Metrics without digests are a traceability gap, not a scoring defect:
    /// packaging may proceed, `--strict` refuses.
    #[test]
    fn c12_warns_when_alignment_evidence_is_not_reproducible() {
        let mut req = req_from_models();
        req.rubric = Some(rubric_spec(0, 0.9655, false));
        let r = run("qa", &req, &GateOptions::default());
        assert_eq!(status_of(&r, "C12").status, CheckStatus::Warn);
        assert!(r.passed);
        assert!(!r.strict_passed);
    }

    /// Over-credit blocks packaging outright — it is the direction a policy can
    /// exploit for reward it did not earn.
    #[test]
    fn c12_fails_on_over_credit() {
        let mut req = req_from_models();
        req.rubric = Some(rubric_spec(1, 0.9655, true));
        let r = run("qa", &req, &GateOptions::default());
        let c12 = status_of(&r, "C12");
        assert_eq!(c12.status, CheckStatus::Fail, "{}", c12.detail);
        assert!(!r.passed);
    }

    #[test]
    fn c12_fails_on_low_agreement_and_on_a_malformed_contract() {
        let mut req = req_from_models();
        req.rubric = Some(rubric_spec(0, 0.42, true));
        assert_eq!(
            status_of(&run("qa", &req, &GateOptions::default()), "C12").status,
            CheckStatus::Fail
        );

        let mut malformed = rubric_spec(0, 0.9655, true);
        malformed.schema_version = "99".into();
        req.rubric = Some(malformed);
        let report = run("qa", &req, &GateOptions::default());
        let c12 = status_of(&report, "C12");
        assert_eq!(c12.status, CheckStatus::Fail);
        assert!(c12.detail.contains("invalid rubric contract"), "{}", c12.detail);
    }

    #[test]
    fn c13_is_skipped_without_a_rubric() {
        let req = req_from_models();
        let r = run("coding-env", &req, &GateOptions::default());
        assert_eq!(status_of(&r, "C13").status, CheckStatus::Skip);
    }

    /// The case C13 exists for: a perfectly aligned rubric whose rules are named
    /// by library and therefore cannot be fetched.
    #[test]
    fn c13_warns_when_the_gold_standard_is_only_named() {
        let mut req = req_from_models();
        req.rubric = Some(rubric_spec(0, 0.9655, false));
        let r = run("qa", &req, &GateOptions::default());
        assert_eq!(status_of(&r, "C12").status, CheckStatus::Warn);
        let c13 = status_of(&r, "C13");
        assert_eq!(c13.status, CheckStatus::Warn);
        assert!(c13.detail.contains("library"), "{}", c13.detail);
        assert!(r.passed, "an undistributable gold standard must not block packaging");
        assert!(!r.strict_passed);
    }

    #[test]
    fn c13_passes_for_a_pinned_runnable_rule_package() {
        let mut req = req_from_models();
        req.rubric = Some(rubric_spec(0, 0.9655, true));
        let r = run("qa", &req, &GateOptions::default());
        let c13 = status_of(&r, "C13");
        assert_eq!(c13.status, CheckStatus::Pass, "{}", c13.detail);
        assert!(c13.detail.contains("qa_rubric.py"), "{}", c13.detail);
        assert!(c13.detail.contains("entrypoint="), "{}", c13.detail);
    }

    /// Downloadable is not the same as runnable: bytes without an entrypoint can
    /// be read but not executed, so the agreement still cannot be re-derived.
    #[test]
    fn c13_warns_when_the_rules_cannot_be_invoked_or_installed() {
        let mut req = req_from_models();
        let mut spec = rubric_spec(0, 0.9655, true);
        spec.reference_scorer.as_mut().unwrap().entrypoint = None;
        req.rubric = Some(spec);
        let no_entrypoint = run("qa", &req, &GateOptions::default());
        let c13 = status_of(&no_entrypoint, "C13");
        assert_eq!(c13.status, CheckStatus::Warn);
        assert!(c13.detail.contains("not run"), "{}", c13.detail);

        let mut spec = rubric_spec(0, 0.9655, true);
        spec.reference_scorer.as_mut().unwrap().requires.clear();
        req.rubric = Some(spec);
        let no_requires = run("qa", &req, &GateOptions::default());
        let c13 = status_of(&no_requires, "C13");
        assert_eq!(c13.status, CheckStatus::Warn);
        assert!(c13.detail.contains("vendored"), "{}", c13.detail);
    }

    /// A digest that cannot be parsed will never verify, so the coordinate is
    /// unusable rather than merely incomplete.
    #[test]
    fn c13_fails_on_an_unverifiable_coordinate() {
        let mut req = req_from_models();
        let mut spec = rubric_spec(0, 0.9655, true);
        {
            let s = spec.reference_scorer.as_mut().unwrap();
            s.digest = "deadbeef".into();
            s.package_ref = "qa-rubric-scorer".into();
        }
        req.rubric = Some(spec);
        let r = run("qa", &req, &GateOptions::default());
        let c13 = status_of(&r, "C13");
        assert_eq!(c13.status, CheckStatus::Fail, "{}", c13.detail);
        assert!(c13.detail.contains("never be verified"), "{}", c13.detail);
        assert!(!r.passed);
    }
}
