//! SWE-smith official harness reward adapter.
//!
//! When `UENV_SWE_SMITH_EVAL_CMD` is configured, Worker delegates final Smith
//! `resolved` to an external command, typically `python -m swesmith.harness.eval`
//! wrapped by `scripts/eval_swesmith_official_reward.py`. The Rust pytest parser
//! remains a fallback and diagnostic path only.

use std::io::Write;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::swe::dataset::SweInstance;
use crate::swe::grader::GradeResult;

type DynErr = Box<dyn std::error::Error + Send + Sync>;

fn run_shell(cmd_line: &str) -> Command {
    #[cfg(windows)]
    {
        let mut c = Command::new("cmd");
        c.args(["/C", cmd_line]);
        c
    }
    #[cfg(not(windows))]
    {
        let mut c = Command::new("sh");
        c.args(["-c", cmd_line]);
        c
    }
}

pub fn try_external_smith_grade(
    instance: &SweInstance,
    model_patch: &str,
    fail_to_pass: &[String],
    pass_to_pass: &[String],
) -> Result<Option<GradeResult>, DynErr> {
    let cmd_line = match std::env::var("UENV_SWE_SMITH_EVAL_CMD") {
        Ok(v) if !v.trim().is_empty() => v,
        _ => return Ok(None),
    };

    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or_default();
    let dir =
        std::env::temp_dir().join(format!("uenv-swesmith-eval-{}-{nonce}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    let instance_path = dir.join("instance.json");
    let patch_path = dir.join("model.patch");
    std::fs::write(&instance_path, serde_json::to_vec(instance)?)?;
    {
        let mut f = std::fs::File::create(&patch_path)?;
        f.write_all(model_patch.as_bytes())?;
    }

    let output = run_shell(&cmd_line)
        .env("UENV_SWE_INSTANCE_ID", &instance.instance_id)
        .env("UENV_SWE_INSTANCE_JSON", &instance_path)
        .env("UENV_SWE_MODEL_PATCH", &patch_path)
        .output()
        .map_err(|e| format!("UENV_SWE_SMITH_EVAL_CMD spawn failed: {e}"))?;
    let _ = std::fs::remove_dir_all(&dir);

    let stdout = String::from_utf8_lossy(&output.stdout);
    if let Ok(parsed) = serde_json::from_str::<SmithEvalJson>(&stdout) {
        return Ok(Some(parsed.into_grade(fail_to_pass, pass_to_pass)));
    }
    Err(format!(
        "UENV_SWE_SMITH_EVAL_CMD returned non-JSON status={} stdout={} stderr={}",
        output.status,
        stdout,
        String::from_utf8_lossy(&output.stderr)
    )
    .into())
}

#[derive(serde::Deserialize)]
struct SmithEvalJson {
    resolved: bool,
    #[serde(default)]
    reward: Option<f64>,
    #[serde(default)]
    per_test: Vec<(String, bool)>,
}

impl SmithEvalJson {
    fn into_grade(self, _fail_to_pass: &[String], _pass_to_pass: &[String]) -> GradeResult {
        let reward = self.reward.unwrap_or(if self.resolved { 1.0 } else { 0.0 });
        let per_test = self.per_test;
        GradeResult {
            resolved: self.resolved,
            reward,
            per_test,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returns_none_when_env_unset() {
        let prev = std::env::var("UENV_SWE_SMITH_EVAL_CMD").ok();
        unsafe { std::env::remove_var("UENV_SWE_SMITH_EVAL_CMD") };
        let inst = SweInstance {
            instance_id: "owner__repo.abcdef12.case__x".into(),
            repo: "swesmith/owner__repo.abcdef12".into(),
            version: "smith".into(),
            base_commit: String::new(),
            environment_setup_commit: String::new(),
            problem_statement: String::new(),
            patch: String::new(),
            test_patch: String::new(),
            fail_to_pass: vec![],
            pass_to_pass: vec![],
            benchmark_variant: Some("smith".into()),
            image_cache_key: None,
            test_cmd: None,
            install_cmd: None,
            setup_cmd: None,
            pre_test_cmd: None,
        };
        let r = try_external_smith_grade(&inst, "", &[], &[]).unwrap();
        assert!(r.is_none());
        if let Some(v) = prev {
            unsafe { std::env::set_var("UENV_SWE_SMITH_EVAL_CMD", v) };
        }
    }
}
