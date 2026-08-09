//! Generic external reward adapter driven by [`BenchmarkRuntimeContract`].
//!
//! The command is configured indirectly via an environment variable named by
//! `reward.command_env`. Worker provides the same stable inputs for every SWE
//! benchmark so new EnvPackages can bring their own scorer without another
//! Worker code change.

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

pub fn try_external_contract_grade(
    instance: &SweInstance,
    model_patch: &str,
    test_output: &str,
    fail_to_pass: &[String],
    pass_to_pass: &[String],
    command_env: Option<&str>,
) -> Result<Option<GradeResult>, DynErr> {
    let Some(command_env) = command_env.filter(|s| !s.trim().is_empty()) else {
        return Ok(None);
    };
    let cmd_line = match std::env::var(command_env) {
        Ok(v) if !v.trim().is_empty() => v,
        _ => return Ok(None),
    };

    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or_default();
    let dir = std::env::temp_dir().join(format!(
        "uenv-swe-contract-eval-{}-{nonce}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir)?;
    let instance_path = dir.join("instance.json");
    let patch_path = dir.join("model.patch");
    let output_path = dir.join("test-output.txt");
    std::fs::write(&instance_path, serde_json::to_vec(instance)?)?;
    {
        let mut f = std::fs::File::create(&patch_path)?;
        f.write_all(model_patch.as_bytes())?;
    }
    {
        let mut f = std::fs::File::create(&output_path)?;
        f.write_all(test_output.as_bytes())?;
    }

    let output = run_shell(&cmd_line)
        .env("UENV_SWE_INSTANCE_ID", &instance.instance_id)
        .env("UENV_SWE_BENCHMARK_VARIANT", instance.variant().as_str())
        .env("UENV_SWE_WORKSPACE_DIR", instance.workspace_dir())
        .env("UENV_SWE_INSTANCE_JSON", &instance_path)
        .env("UENV_SWE_MODEL_PATCH", &patch_path)
        .env("UENV_SWE_TEST_OUTPUT_PATH", &output_path)
        .env("UENV_SWE_TEST_OUTPUT", test_output)
        .output()
        .map_err(|e| format!("{command_env} spawn failed: {e}"))?;
    let _ = std::fs::remove_dir_all(&dir);

    let stdout = String::from_utf8_lossy(&output.stdout);
    if let Ok(parsed) = serde_json::from_str::<ExternalEvalJson>(&stdout) {
        return Ok(Some(parsed.into_grade(fail_to_pass, pass_to_pass)));
    }
    Err(format!(
        "{command_env} returned non-JSON status={} stdout={} stderr={}",
        output.status,
        stdout,
        String::from_utf8_lossy(&output.stderr)
    )
    .into())
}

#[derive(serde::Deserialize)]
struct ExternalEvalJson {
    resolved: bool,
    #[serde(default)]
    reward: Option<f64>,
    #[serde(default)]
    per_test: Vec<(String, bool)>,
}

impl ExternalEvalJson {
    fn into_grade(self, fail_to_pass: &[String], pass_to_pass: &[String]) -> GradeResult {
        let reward = self.reward.unwrap_or(if self.resolved { 1.0 } else { 0.0 });
        let per_test = if self.per_test.is_empty() {
            fail_to_pass
                .iter()
                .chain(pass_to_pass.iter())
                .map(|id| (id.clone(), self.resolved))
                .collect()
        } else {
            self.per_test
        };
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
    fn returns_none_when_command_env_missing() {
        let inst = SweInstance {
            instance_id: "owner__repo-1".into(),
            repo: "owner/repo".into(),
            version: String::new(),
            base_commit: String::new(),
            environment_setup_commit: String::new(),
            problem_statement: String::new(),
            patch: String::new(),
            test_patch: String::new(),
            fail_to_pass: vec![],
            pass_to_pass: vec![],
            benchmark_variant: None,
            image_cache_key: None,
            test_cmd: None,
            install_cmd: None,
            setup_cmd: None,
            pre_test_cmd: None,
            runtime_contract: None,
        };
        assert!(
            try_external_contract_grade(&inst, "", "", &[], &[], Some("UENV_MISSING_CMD"))
                .unwrap()
                .is_none()
        );
    }
}
