//! Rubric (gold-standard scoring contract) rules.
//!
//! A verification-type Task Environment (`qa`) computes its reward from a rule
//! rather than from program execution, so the rule is part of the environment
//! contract: a training run has to be able to state *which* gold standard its
//! rewards were aligned against, and an operator has to be able to tell whether
//! a published version is safe to make the default.
//!
//! Two things live here:
//!
//! 1. **Structural validation** — is the declared rubric internally coherent and
//!    consistent with the version's `config_schema` dataset routing?
//! 2. **The promotion gate** — may this version resolve as `versions/latest`?
//!
//! The gate keys on *over-credit* (production scorer rewards where the reference
//! implementation does not). That asymmetry is deliberate: over-credit is the
//! direction a policy can exploit, so it blocks; under-credit only loses recall,
//! so it is recorded and does not block.
//!
//! A barred version is still published and still fetchable by exact version —
//! only `latest` skips it. Rejecting the publish outright would push people to
//! publish without a rubric at all, which is strictly worse for auditability.

use uenv_hub_types::{RubricSpec, ValidationReport, RUBRIC_SCHEMA_VERSION};
#[cfg(test)]
use uenv_hub_types::RubricScorerRef;

/// Accepted `known_gaps[].severity` values.
const GAP_SEVERITIES: &[&str] = &["too_strict", "too_lenient", "intentional"];

/// Thresholds for the promotion gate.
#[derive(Debug, Clone)]
pub struct GateOptions {
    /// Minimum agreement rate against the reference scorer.
    pub min_agreement_rate: f64,
    /// Largest tolerated over-credit case count.
    pub max_over_credit: i64,
    /// When `false`, findings are recorded but never bar promotion.
    pub enforce: bool,
}

impl Default for GateOptions {
    /// Mirrors `verify_qa_rubric_alignment.py`'s defaults (`--min-agreement 0.95`,
    /// `--max-over-credit 0`) so the Hub gate and the aligner agree on what
    /// "aligned" means; a corpus run that passes locally is not re-judged here.
    fn default() -> Self {
        Self {
            min_agreement_rate: 0.95,
            max_over_credit: 0,
            enforce: true,
        }
    }
}

/// Result of applying the promotion gate to one version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateOutcome {
    /// Whether the version may resolve as `versions/latest`.
    pub eligible: bool,
    /// Findings, in publish-response and manifest order.
    pub notes: Vec<String>,
}

impl GateOutcome {
    /// The unconstrained outcome: promotable, nothing to report.
    pub fn clear() -> Self {
        Self {
            eligible: true,
            notes: Vec::new(),
        }
    }
}

/// Validate a rubric block's internal coherence.
///
/// `dataset_keys`, when supplied, are the dataset values the version's
/// `config_schema` accepts; a rubric that scores a dataset the environment
/// cannot be asked to run (or omits one it can) is a real drift signal, so it is
/// reported rather than ignored.
pub fn validate(
    rubric: &RubricSpec,
    dataset_keys: Option<&[String]>,
    report: &mut ValidationReport,
) {
    if rubric.schema_version.trim() != RUBRIC_SCHEMA_VERSION {
        report.push_error(
            "rubric.schema_version",
            format!(
                "unsupported rubric schema '{}'; this Hub accepts '{}'",
                rubric.schema_version, RUBRIC_SCHEMA_VERSION
            ),
        );
    }

    if rubric
        .production_scorer
        .as_deref()
        .unwrap_or("")
        .trim()
        .is_empty()
    {
        report.push_warning(
            "rubric.production_scorer",
            "name the scorer that actually produces rewards (e.g. \
             'uenv-math-plugin/score_action') so a trajectory can be traced to it",
        );
    }

    validate_reference_scorer(rubric, report);

    match &rubric.alignment {
        None => report.push_warning(
            "rubric.alignment",
            "no alignment evidence declared; the version cannot state which gold \
             standard its rewards match",
        ),
        Some(alignment) => {
            for (field, digest) in [
                ("rubric.alignment.corpus_digest", &alignment.corpus_digest),
                ("rubric.alignment.report_digest", &alignment.report_digest),
            ] {
                match digest {
                    Some(d) if !is_sha256(d) => {
                        report.push_error(field, "must be a 'sha256:<hex>' digest");
                    }
                    None => report.push_warning(
                        field,
                        "absent; without it the alignment claim is not reproducible",
                    ),
                    _ => {}
                }
            }

            if let Some(pkg) = &alignment.package_ref {
                if !pkg.contains('@') {
                    report.push_error(
                        "rubric.alignment.package_ref",
                        "must be of the form 'package_id@version'",
                    );
                }
            }

            match &alignment.metrics {
                None => report.push_warning(
                    "rubric.alignment.metrics",
                    "no agreement metrics declared; the promotion gate cannot judge \
                     this version and will treat it as unmeasured",
                ),
                Some(m) => {
                    if !(0.0..=1.0).contains(&m.agreement_rate) {
                        report.push_error(
                            "rubric.alignment.metrics.agreement_rate",
                            "must be a fraction in [0, 1]",
                        );
                    }
                    if m.over_credit_count < 0 {
                        report.push_error(
                            "rubric.alignment.metrics.over_credit_count",
                            "must not be negative",
                        );
                    }
                    if m.under_credit_count < 0 {
                        report.push_error(
                            "rubric.alignment.metrics.under_credit_count",
                            "must not be negative",
                        );
                    }
                    // `agreed` and `total` are optional, but if both are given they
                    // must be consistent with each other and with the rate.
                    if let (Some(total), Some(agreed)) = (m.total, m.agreed) {
                        if total < 0 || agreed < 0 {
                            report.push_error(
                                "rubric.alignment.metrics.total",
                                "counts must not be negative",
                            );
                        } else if agreed > total {
                            report.push_error(
                                "rubric.alignment.metrics.agreed",
                                "cannot exceed 'total'",
                            );
                        } else if total > 0 {
                            let derived = agreed as f64 / total as f64;
                            if (derived - m.agreement_rate).abs() > 0.005 {
                                report.push_error(
                                    "rubric.alignment.metrics.agreement_rate",
                                    format!(
                                        "inconsistent with agreed/total ({agreed}/{total} = {derived:.4})"
                                    ),
                                );
                            }
                        }
                    }
                    if let Some(total) = m.total {
                        if total > 0 && m.over_credit_count + m.under_credit_count > total {
                            report.push_error(
                                "rubric.alignment.metrics",
                                "over_credit_count + under_credit_count exceeds total",
                            );
                        }
                    }
                }
            }
        }
    }

    for (i, gap) in rubric.known_gaps.iter().enumerate() {
        if gap.id.trim().is_empty() {
            report.push_error(format!("rubric.known_gaps[{i}].id"), "must not be empty");
        }
        if !GAP_SEVERITIES.contains(&gap.severity.as_str()) {
            report.push_error(
                format!("rubric.known_gaps[{i}].severity"),
                format!("must be one of {}", GAP_SEVERITIES.join(" / ")),
            );
        }
    }

    if rubric.datasets.is_empty() {
        report.push_warning(
            "rubric.datasets",
            "no per-dataset scorer routing declared; reward semantics are then \
             undocumented per benchmark",
        );
    }

    if let Some(keys) = dataset_keys {
        for name in rubric.datasets.keys() {
            if !keys.iter().any(|k| k == name) {
                report.push_error(
                    format!("rubric.datasets.{name}"),
                    "scores a dataset that config_schema does not accept",
                );
            }
        }
        for key in keys {
            if !rubric.datasets.contains_key(key) {
                report.push_warning(
                    format!("rubric.datasets.{key}"),
                    "dataset is accepted by config_schema but has no declared scorer",
                );
            }
        }
    }
}

/// Validate the reference-scorer coordinate.
///
/// `backend: "verifiers+math_verify"` names a library, not a rule package. Which
/// answer a rubric accepts depends on the extraction rules written on top of that
/// library — GSM8K's `####` marker versus a boxed-only parser change every score
/// in the corpus — so two hosts can agree on the backend string and still reward
/// differently. Requiring an artifact coordinate makes the claim byte-checkable.
fn validate_reference_scorer(rubric: &RubricSpec, report: &mut ValidationReport) {
    let Some(scorer) = &rubric.reference_scorer else {
        // Only a warning: environments published before this field existed remain
        // valid, and blocking them would make the field's introduction a breaking
        // change for a claim that is stricter than what the gate already enforces.
        if rubric.alignment.is_some() {
            report.push_warning(
                "rubric.reference_scorer",
                "alignment evidence is declared but the gold-standard rule package is \
                 not; a consumer can read the agreement number without being able to \
                 fetch the rules it was measured against",
            );
        }
        return;
    };

    if !scorer.package_ref.contains('@') {
        report.push_error(
            "rubric.reference_scorer.package_ref",
            "must be of the form 'package_id@version'",
        );
    }
    if scorer.artifact.trim().is_empty() {
        report.push_error(
            "rubric.reference_scorer.artifact",
            "name the artifact inside the package that holds the rules \
             (e.g. 'qa_rubric.py')",
        );
    }
    if !is_sha256(&scorer.digest) {
        report.push_error(
            "rubric.reference_scorer.digest",
            "must be a 'sha256:<hex>' digest of the scorer source",
        );
    }
    // An entrypoint is what turns "these bytes exist" into "this is how you run
    // them"; without it a consumer has to guess a callable name.
    if scorer
        .entrypoint
        .as_deref()
        .unwrap_or("")
        .trim()
        .is_empty()
    {
        report.push_warning(
            "rubric.reference_scorer.entrypoint",
            "no callable declared; give 'module:function' so a consumer can execute \
             the rules rather than only read them",
        );
    } else if let Some(ep) = &scorer.entrypoint {
        if !ep.contains(':') {
            report.push_error(
                "rubric.reference_scorer.entrypoint",
                "must be of the form 'module:callable'",
            );
        }
    }
    if scorer.requires.is_empty() {
        report.push_warning(
            "rubric.reference_scorer.requires",
            "no Python requirements declared; an air-gapped consumer cannot tell what \
             it must have vendored to execute the rules",
        );
    }
}

/// Apply the promotion gate.
///
/// A version with no rubric is unconstrained — most environments (`code`, `swe`)
/// reward by execution and have nothing to align. The gate only judges versions
/// that opted into the rubric contract.
pub fn gate(rubric: Option<&RubricSpec>, opts: &GateOptions) -> GateOutcome {
    let Some(rubric) = rubric else {
        return GateOutcome::clear();
    };
    let mut notes = Vec::new();
    let mut blocked = false;

    match rubric.alignment.as_ref().and_then(|a| a.metrics.as_ref()) {
        None => {
            notes.push(
                "rubric declared without alignment metrics: cannot confirm the production \
                 scorer matches the reference implementation"
                    .to_string(),
            );
            blocked = true;
        }
        Some(m) => {
            if m.over_credit_count > opts.max_over_credit {
                notes.push(format!(
                    "over-credit cases {} exceed the allowed {}: the scorer rewards answers the \
                     reference rejects, which a policy can exploit",
                    m.over_credit_count, opts.max_over_credit
                ));
                blocked = true;
            }
            if m.agreement_rate < opts.min_agreement_rate {
                notes.push(format!(
                    "agreement rate {:.4} is below the required {:.4}",
                    m.agreement_rate, opts.min_agreement_rate
                ));
                blocked = true;
            }
        }
    }

    if blocked && !opts.enforce {
        notes.push(
            "promotion gate is disabled on this Hub; recorded as advisory only".to_string(),
        );
        return GateOutcome {
            eligible: true,
            notes,
        };
    }

    GateOutcome {
        eligible: !blocked,
        notes,
    }
}

/// Dataset values a `config_schema` accepts, read from the conventional
/// `properties.dataset.enum` used by the standardized env manifests.
pub fn dataset_keys_from_config_schema(schema: &serde_json::Value) -> Option<Vec<String>> {
    let values = schema
        .get("properties")?
        .get("dataset")?
        .get("enum")?
        .as_array()?;
    let keys: Vec<String> = values
        .iter()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect();
    if keys.is_empty() {
        None
    } else {
        Some(keys)
    }
}

fn is_sha256(digest: &str) -> bool {
    let Some(hex) = digest.strip_prefix("sha256:") else {
        return false;
    };
    hex.len() == 64 && hex.chars().all(|c| c.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::BTreeMap;
    use uenv_hub_types::{RubricAlignment, RubricDataset, RubricGap, RubricMetrics, Severity};

    fn digest(hex_char: char) -> String {
        format!("sha256:{}", String::from(hex_char).repeat(64))
    }

    fn has_error(report: &ValidationReport, location: &str) -> bool {
        report
            .issues
            .iter()
            .any(|i| i.severity == Severity::Error && i.location == location)
    }

    fn has_error_containing(report: &ValidationReport, needle: &str) -> bool {
        report
            .issues
            .iter()
            .any(|i| i.severity == Severity::Error && i.location.contains(needle))
    }

    fn has_warning(report: &ValidationReport, location: &str) -> bool {
        report
            .issues
            .iter()
            .any(|i| i.severity == Severity::Warning && i.location == location)
    }

    fn aligned_metrics() -> RubricMetrics {
        RubricMetrics {
            total: Some(58),
            agreed: Some(56),
            agreement_rate: 56.0 / 58.0,
            over_credit_count: 0,
            under_credit_count: 2,
            verifiers_version: Some("0.1.3".into()),
            math_verify_version: Some("0.8.0".into()),
        }
    }

    fn spec(metrics: Option<RubricMetrics>) -> RubricSpec {
        let mut datasets = BTreeMap::new();
        datasets.insert(
            "gsm8k".to_string(),
            RubricDataset {
                scorer: Some("gsm8k".into()),
                notes: None,
            },
        );
        RubricSpec {
            schema_version: "1".into(),
            backend: Some("verifiers+math_verify".into()),
            production_scorer: Some("uenv-math-plugin/score_action".into()),
            reference_scorer: Some(RubricScorerRef {
                package_ref: "qa-rubric-scorer@0.1.0".into(),
                artifact: "qa_rubric.py".into(),
                digest: digest('c'),
                entrypoint: Some("qa_rubric:score".into()),
                rubric_classes: vec!["Rubric".into(), "MathRubric".into()],
                requires: vec!["verifiers".into(), "math-verify".into()],
            }),
            alignment: Some(RubricAlignment {
                corpus_id: Some("qa_rubric_corpus@2026-07-25".into()),
                corpus_digest: Some(digest('a')),
                report_digest: Some(digest('b')),
                package_ref: Some("qa-rubric-align@0.1.0".into()),
                metrics,
            }),
            datasets,
            known_gaps: vec![RubricGap {
                id: "natural_language_without_hash".into(),
                severity: "too_strict".into(),
                notes: None,
            }],
        }
    }

    #[test]
    fn aligned_rubric_validates_and_promotes() {
        let mut report = ValidationReport::ok();
        validate(&spec(Some(aligned_metrics())), None, &mut report);
        assert!(report.valid, "{:?}", report.issues);

        let outcome = gate(Some(&spec(Some(aligned_metrics()))), &GateOptions::default());
        assert!(outcome.eligible);
        assert!(outcome.notes.is_empty(), "{:?}", outcome.notes);
    }

    #[test]
    fn over_credit_bars_promotion_but_under_credit_does_not() {
        let mut over = aligned_metrics();
        over.over_credit_count = 1;
        let outcome = gate(Some(&spec(Some(over))), &GateOptions::default());
        assert!(!outcome.eligible);
        assert!(
            outcome.notes[0].contains("over-credit"),
            "{:?}",
            outcome.notes
        );

        let mut under = aligned_metrics();
        under.under_credit_count = 20;
        assert!(gate(Some(&spec(Some(under))), &GateOptions::default()).eligible);
    }

    #[test]
    fn low_agreement_bars_promotion() {
        let metrics = RubricMetrics {
            total: Some(100),
            agreed: Some(80),
            agreement_rate: 0.80,
            over_credit_count: 0,
            under_credit_count: 20,
            ..aligned_metrics()
        };
        let outcome = gate(Some(&spec(Some(metrics))), &GateOptions::default());
        assert!(!outcome.eligible);
        assert!(
            outcome.notes.iter().any(|n| n.contains("agreement rate")),
            "{:?}",
            outcome.notes
        );
    }

    #[test]
    fn missing_metrics_is_not_treated_as_aligned() {
        let outcome = gate(Some(&spec(None)), &GateOptions::default());
        assert!(
            !outcome.eligible,
            "an unmeasured rubric must not silently become latest"
        );
    }

    #[test]
    fn no_rubric_is_unconstrained() {
        assert!(gate(None, &GateOptions::default()).eligible);
    }

    #[test]
    fn disabled_gate_records_findings_without_blocking() {
        let mut over = aligned_metrics();
        over.over_credit_count = 3;
        let opts = GateOptions {
            enforce: false,
            ..GateOptions::default()
        };
        let outcome = gate(Some(&spec(Some(over))), &opts);
        assert!(outcome.eligible);
        assert!(outcome.notes.iter().any(|n| n.contains("disabled")));
    }

    #[test]
    fn draft_metric_key_names_are_accepted_on_input() {
        // The design draft used agreement / too_lenient / too_strict; the aligner
        // emits agreement_rate / over_credit_count / under_credit_count. Both must
        // deserialize so neither document nor tool needs a manual translation.
        let m: RubricMetrics = serde_json::from_value(json!({
            "agreement": 0.9655,
            "too_lenient": 0,
            "too_strict": 2
        }))
        .expect("draft key names must deserialize");
        assert_eq!(m.over_credit_count, 0);
        assert_eq!(m.under_credit_count, 2);
        assert!((m.agreement_rate - 0.9655).abs() < 1e-9);

        // And the canonical names round-trip.
        let json = serde_json::to_value(&m).unwrap();
        assert_eq!(json["over_credit_count"], 0);
        assert_eq!(json["agreement_rate"], 0.9655);
    }

    #[test]
    fn inconsistent_metrics_are_rejected() {
        let metrics = RubricMetrics {
            total: Some(58),
            agreed: Some(58),
            agreement_rate: 0.5, // contradicts 58/58
            ..aligned_metrics()
        };
        let mut report = ValidationReport::ok();
        validate(&spec(Some(metrics)), None, &mut report);
        assert!(
            has_error_containing(&report, "agreement_rate"),
            "{:?}",
            report.issues
        );
    }

    #[test]
    fn bad_digest_and_severity_are_rejected() {
        let mut s = spec(Some(aligned_metrics()));
        s.alignment.as_mut().unwrap().corpus_digest = Some("deadbeef".into());
        s.known_gaps[0].severity = "cosmetic".into();
        let mut report = ValidationReport::ok();
        validate(&s, None, &mut report);
        assert!(has_error(&report, "rubric.alignment.corpus_digest"));
        assert!(has_error(&report, "rubric.known_gaps[0].severity"));
    }

    #[test]
    fn dataset_routing_is_cross_checked_against_config_schema() {
        let schema = json!({
            "type": "object",
            "properties": { "dataset": { "enum": ["gsm8k", "pubmedqa"] } }
        });
        let keys = dataset_keys_from_config_schema(&schema).expect("enum present");
        assert_eq!(keys, vec!["gsm8k", "pubmedqa"]);

        let mut s = spec(Some(aligned_metrics()));
        s.datasets
            .insert("not-a-dataset".to_string(), RubricDataset::default());
        let mut report = ValidationReport::ok();
        validate(&s, Some(&keys), &mut report);
        // Unknown dataset is an error; the accepted-but-unscored one is a warning.
        assert!(has_error(&report, "rubric.datasets.not-a-dataset"));
        assert!(has_warning(&report, "rubric.datasets.pubmedqa"));
    }

    #[test]
    fn unsupported_schema_version_is_rejected() {
        let mut s = spec(Some(aligned_metrics()));
        s.schema_version = "2".into();
        let mut report = ValidationReport::ok();
        validate(&s, None, &mut report);
        assert!(has_error(&report, "rubric.schema_version"));
    }
}
