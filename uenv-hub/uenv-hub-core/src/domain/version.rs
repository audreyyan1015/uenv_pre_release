//! Semantic-version helpers: parsing, normalization for DB ordering, and
//! constraint resolution.
//!
//! The classic footgun this module guards against is lexicographic ordering of
//! version strings (`"1.10.0" < "1.9.0"`). We store a zero-padded
//! `version_normalized` column so `ORDER BY` is correct, and use the `semver`
//! crate for precise comparison / constraint resolution.

use crate::error::{HubError, Result};
use semver::{Version, VersionReq};

/// Width used when zero-padding each numeric component.
const PAD: usize = 5;

/// Parse a semver string, mapping failures to [`HubError::InvalidVersion`].
pub fn parse(version: &str) -> Result<Version> {
    Version::parse(version).map_err(|_| HubError::InvalidVersion(version.to_string()))
}

/// Produce a lexicographically-sortable normalized form.
///
/// `1.2.3`        -> `00001.00002.00003~`
/// `1.2.3-alpha`  -> `00001.00002.00003-alpha`
///
/// The trailing `~` (0x7E) ensures a release sorts *after* any pre-release of
/// the same core (since `-` is 0x2D), matching semver precedence rules well
/// enough for `ORDER BY`. Exact comparisons still go through `semver::Version`.
pub fn normalize(version: &str) -> Result<String> {
    let v = parse(version)?;
    let core = format!("{:0w$}.{:0w$}.{:0w$}", v.major, v.minor, v.patch, w = PAD);
    if v.pre.is_empty() {
        Ok(format!("{core}~"))
    } else {
        Ok(format!("{core}-{}", v.pre.as_str()))
    }
}

/// Validate a version-constraint string (e.g. `^1.0`, `>=2.0, <3.0`).
pub fn parse_constraint(constraint: &str) -> Result<VersionReq> {
    VersionReq::parse(constraint).map_err(|_| HubError::InvalidConstraint(constraint.to_string()))
}

/// Pick the highest version from `candidates` satisfying `constraint`.
///
/// Yanked versions should be filtered out by the caller before calling this.
/// Returns the chosen version string, or `None` if nothing matches.
pub fn resolve<'a, I>(candidates: I, constraint: &str) -> Result<Option<String>>
where
    I: IntoIterator<Item = &'a str>,
{
    let req = parse_constraint(constraint)?;
    let mut best: Option<Version> = None;
    for raw in candidates {
        let Ok(v) = Version::parse(raw) else { continue };
        if req.matches(&v) {
            match &best {
                Some(cur) if *cur >= v => {}
                _ => best = Some(v),
            }
        }
    }
    Ok(best.map(|v| v.to_string()))
}

/// Pick the highest version overall (ignoring pre-releases unless that's all
/// there is). Used for `latest`.
pub fn latest<'a, I>(candidates: I) -> Option<String>
where
    I: IntoIterator<Item = &'a str>,
{
    let mut stable: Option<Version> = None;
    let mut any: Option<Version> = None;
    for raw in candidates {
        let Ok(v) = Version::parse(raw) else { continue };
        if any.as_ref().map(|c| v > *c).unwrap_or(true) {
            any = Some(v.clone());
        }
        if v.pre.is_empty() && stable.as_ref().map(|c| v > *c).unwrap_or(true) {
            stable = Some(v);
        }
    }
    stable.or(any).map(|v| v.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_orders_numerically() {
        // The whole point: 1.9.0 must sort before 1.10.0.
        let a = normalize("1.9.0").unwrap();
        let b = normalize("1.10.0").unwrap();
        assert!(a < b, "{a} should sort before {b}");
    }

    #[test]
    fn release_sorts_after_prerelease() {
        let pre = normalize("1.0.0-alpha").unwrap();
        let rel = normalize("1.0.0").unwrap();
        assert!(pre < rel, "{pre} should sort before {rel}");
    }

    #[test]
    fn resolve_caret() {
        let versions = ["1.0.0", "1.2.0", "1.5.3", "2.0.0"];
        let got = resolve(versions, "^1.0").unwrap();
        assert_eq!(got.as_deref(), Some("1.5.3"));
    }

    #[test]
    fn resolve_range() {
        let versions = ["1.0.0", "2.1.0", "2.9.9", "3.0.0"];
        let got = resolve(versions, ">=2.0, <3.0").unwrap();
        assert_eq!(got.as_deref(), Some("2.9.9"));
    }

    #[test]
    fn latest_prefers_stable() {
        let versions = ["1.0.0", "2.0.0-rc1"];
        assert_eq!(latest(versions).as_deref(), Some("1.0.0"));
    }

    #[test]
    fn invalid_version_rejected() {
        assert!(parse("not-a-version").is_err());
    }
}
