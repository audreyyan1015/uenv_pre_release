use std::sync::Arc;
use std::time::Instant;

use sha2::{Digest, Sha256};

use crate::episode::model_client::ModelClient;
use crate::llm::LlmConfig;
use crate::episode::payload::build_reset_config;
use crate::episode::reward_engine::RewardEngine;
use crate::plugin::host::PluginHost;
use crate::pool::warmup_pool::WarmupPool;
use crate::proto::v1::{EpisodeRequest, EpisodeResult, ReportType, StepRecord, StreamReport, Trajectory};
use crate::swe::command_policy::CommandPolicyConfig;
use crate::swe::dataset::InstanceStore;
use crate::swe::harness::{run_instance, ContainerRuntime, RunOptions};
use crate::swe::instance_pool::SweInstancePool;

/// SWE-bench episode 的 env_type（DispatchEpisode 路由键）。
pub const SWE_ENV_TYPE: &str = "swe";

#[derive(Clone)]
pub struct EpisodeExecutor {
    warmup_pool: WarmupPool,
    plugin_host: PluginHost,
    model_client: ModelClient,
    reward_engine: RewardEngine,
    /// SWE-bench 实例目录（Hub 下发或本地 fixtures）。
    swe_store: Arc<InstanceStore>,
    swe_runtime: ContainerRuntime,
    /// L2 共享会话池（M2-2）：与 L4 Gateway 共用，native 路径经此 acquire/submit/release。
    /// `None` 时回退一次性 `harness::run_instance`（无池环境，如部分单测）。
    swe_pool: Option<Arc<SweInstancePool>>,
}

#[derive(Clone, Debug)]
pub struct ExecuteContext {
    pub worker_id: String,
    pub worker_capacity: u32,
    pub active_episodes: u32,
}

pub struct ExecuteOutput {
    pub stream_reports: Vec<StreamReport>,
    pub result: EpisodeResult,
    pub reward: f64,
    pub duration_ms: u64,
    pub env_step_duration_ms: u64,
    pub model_callback_duration_ms: u64,
    pub warmup_hit: bool,
}

impl EpisodeExecutor {
    pub fn new(plugin_host: PluginHost, warmup_pool: WarmupPool, llm: LlmConfig) -> Self {
        Self {
            warmup_pool,
            plugin_host,
            model_client: ModelClient::with_config(llm),
            reward_engine: RewardEngine::new(),
            swe_store: Arc::new(InstanceStore::default()),
            swe_runtime: ContainerRuntime::Docker,
            swe_pool: None,
        }
    }

    /// 注入 SWE-bench 实例目录与容器运行时（运行时从 Hub/本地加载）。
    pub fn with_swe_catalog(mut self, store: Arc<InstanceStore>, runtime: ContainerRuntime) -> Self {
        self.swe_store = store;
        self.swe_runtime = runtime;
        self
    }

    /// 注入与 Gateway 共享的 L2 会话池（M2-2）。
    pub fn with_swe_pool(mut self, pool: Arc<SweInstancePool>) -> Self {
        self.swe_pool = Some(pool);
        self
    }

    pub async fn execute_single_round(
        &self,
        episode: &EpisodeRequest,
        ctx: &ExecuteContext,
    ) -> Result<ExecuteOutput, Box<dyn std::error::Error + Send + Sync>> {
        self.execute_episode(episode, ctx).await
    }

    pub async fn execute_episode(
        &self,
        episode: &EpisodeRequest,
        ctx: &ExecuteContext,
    ) -> Result<ExecuteOutput, Box<dyn std::error::Error + Send + Sync>> {
        // SWE-bench 路由：从 Hub 实例镜像拉起容器跑评测，不经 plugin/LLM step 循环。
        if episode.env_type == SWE_ENV_TYPE {
            return self.execute_swe_episode(episode, ctx).await;
        }

        let start = Instant::now();
        let trace_id = episode.correlation_id.clone();
        let max_steps = episode.max_steps.max(1);
        let lease = self
            .warmup_pool
            .acquire(&episode.env_type)
            .await
            .map_err(|err| {
                log_phase_error(&trace_id, &episode.episode_id, "acquire", "ERR_POOL_ACQUIRE_FAILED", &*err);
                err
            })?;
        tracing::info!(
            trace_id = %trace_id,
            episode_id = %episode.episode_id,
            worker_id = %ctx.worker_id,
            phase = "acquire",
            warmup_hit = lease.warmup_hit,
            instance_id = %lease.instance_id,
            msg = "episode_phase"
        );

        let reset_config = build_reset_config(&episode.payload, &episode.reward_config, episode.seed)?;
        let observation = self
            .plugin_host
            .reset(&lease.instance_id, episode.seed, Some(&reset_config))
            .await
            .map_err(|err| {
                log_phase_error(&trace_id, &episode.episode_id, "reset", "ERR_ENV_RESET_FAILED", &*err);
                err
            })?;

        let mut steps = Vec::new();
        let mut stream_reports = Vec::new();
        let mut total_reward = 0.0;
        let mut model_callback_duration_ms = 0u64;
        let mut env_step_duration_ms = 0u64;
        let mut current_observation = observation;
        let mut terminate_reason = "max_steps_reached".to_string();
        let mut last_reward = 0.0;

        for step_index in 1..=max_steps as i32 {
            let model_start = Instant::now();
            let action = self
                .model_client
                .infer_action(
                    &episode.payload,
                    &episode.reward_config,
                    step_index as u32,
                    &episode.model_endpoint,
                )
                .await
                .map_err(|err| {
                    log_phase_error(&trace_id, &episode.episode_id, "model_callback", "ERR_MODEL_CALL_FAILED", &*err);
                    err
                })?;
            model_callback_duration_ms += model_start.elapsed().as_millis() as u64;

            let step_start = Instant::now();
            let step = match self.plugin_host.step(&lease.instance_id, action.clone()).await {
                Ok(step) => step,
                Err(err) => {
                    log_phase_error(&trace_id, &episode.episode_id, "step", "ERR_ENV_STEP_FAILED", &*err);
                    let _ = self.warmup_pool.release(lease.clone()).await;
                    return Err(err);
                }
            };
            let step_duration_ms = step_start.elapsed().as_millis() as u64;
            env_step_duration_ms += step_duration_ms;

            let reward = self.reward_engine.resolve_reward(
                &action,
                &episode.reward_config,
                step.reward,
            )?;
            total_reward += reward;
            last_reward = reward;

            let mut step_info = step.info;
            if let Ok(action_text) = std::str::from_utf8(&action) {
                if !action_text.trim().is_empty() {
                    step_info
                        .entry("response_text".to_string())
                        .or_insert_with(|| action_text.to_string());
                }
            }
            let step_record = StepRecord {
                step_index,
                observation: current_observation.clone(),
                action,
                reward,
                terminated: step.terminated,
                truncated: step.truncated,
                info: step_info,
                duration_ms: step_duration_ms as i64,
            };
            current_observation = step.observation;
            steps.push(step_record.clone());

            stream_reports.push(StreamReport {
                episode_id: episode.episode_id.clone(),
                attempt_id: episode.attempt_id,
                current_step: step_index,
                total_steps: max_steps,
                current_reward: total_reward,
                phase: if step.terminated || step.truncated || step_index == max_steps {
                    "step_complete".to_string()
                } else {
                    "running".to_string()
                },
                last_step: Some(step_record),
                report_type: ReportType::StepComplete as i32,
                step_latency_ms: step_duration_ms as i64,
                model_latency_ms: model_callback_duration_ms as i64,
                worker_active_episodes: ctx.active_episodes as i32,
                worker_capacity: ctx.worker_capacity as i32,
                correlation_id: episode.correlation_id.clone(),
                worker_id: ctx.worker_id.clone(),
                ..Default::default()
            });

            if step.terminated {
                terminate_reason = "terminated".to_string();
                break;
            }
            if step.truncated {
                terminate_reason = "truncated".to_string();
                break;
            }
        }

        self.warmup_pool.release(lease.clone()).await.map_err(|err| {
            log_phase_error(&trace_id, &episode.episode_id, "release", "ERR_POOL_RELEASE_FAILED", &*err);
            err
        })?;

        let total_steps = steps.len() as i32;
        let trajectory = Trajectory {
            steps,
            total_reward,
            total_steps,
        };
        let checksum = checksum_trajectory(&trajectory)?;
        let duration_ms = start.elapsed().as_millis() as u64;
        let result = EpisodeResult {
            episode_id: episode.episode_id.clone(),
            attempt_id: episode.attempt_id,
            status: "completed".to_string(),
            trajectory: Some(trajectory),
            summary: Some(crate::proto::v1::episode_result::Summary {
                total_reward,
                total_steps,
                total_duration_ms: duration_ms as i64,
                terminate_reason,
            }),
            error_code: None,
            error_message: String::new(),
            trajectory_checksum: checksum,
            integrity_verified: true,
        };

        if let Some(last) = stream_reports.last_mut() {
            last.phase = "episode_complete".to_string();
            last.report_type = ReportType::Progress as i32;
        }

        Ok(ExecuteOutput {
            stream_reports,
            result,
            reward: last_reward,
            duration_ms,
            env_step_duration_ms,
            model_callback_duration_ms,
            warmup_hit: lease.warmup_hit,
        })
    }
}

impl EpisodeExecutor {
    /// SWE-bench episode：解析 payload `{instance_id, use_gold_patch}` → 从实例镜像
    /// provision 容器 → reset → 应用 patch → 跑测试 → reward，封装为 `EpisodeResult`。
    async fn execute_swe_episode(
        &self,
        episode: &EpisodeRequest,
        ctx: &ExecuteContext,
    ) -> Result<ExecuteOutput, Box<dyn std::error::Error + Send + Sync>> {
        let start = Instant::now();
        let trace_id = episode.correlation_id.clone();
        let payload: serde_json::Value = if episode.payload.is_empty() {
            serde_json::json!({})
        } else {
            serde_json::from_slice(&episode.payload)
                .map_err(|err| Box::<dyn std::error::Error + Send + Sync>::from(format!("invalid swe payload: {err}")))?
        };
        let instance_id = payload
            .get("instance_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| Box::<dyn std::error::Error + Send + Sync>::from("swe payload missing instance_id"))?
            .to_string();
        let use_gold = payload
            .get("use_gold_patch")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        // plan §6.1 payload：command_mode（Full/RestrictedShell）+ benchmark_variant。
        let mode = payload
            .get("command_mode")
            .and_then(|v| v.as_str())
            .and_then(crate::swe::CommandPolicy::parse)
            .unwrap_or(crate::swe::CommandPolicy::FullShell);
        let variant = payload
            .get("benchmark_variant")
            .and_then(|v| v.as_str())
            .and_then(crate::swe::BenchmarkVariant::parse)
            .unwrap_or_default();

        let instance = self
            .swe_store
            .get(&instance_id)
            .ok_or_else(|| {
                Box::<dyn std::error::Error + Send + Sync>::from(format!(
                    "swe instance_id `{instance_id}` not in catalog (size={})",
                    self.swe_store.len()
                ))
            })?
            .clone();

        tracing::info!(
            trace_id = %trace_id,
            episode_id = %episode.episode_id,
            worker_id = %ctx.worker_id,
            instance_id = %instance_id,
            use_gold_patch = use_gold,
            benchmark_variant = %variant.as_str(),
            command_mode = ?mode,
            phase = "swe_dispatch",
            msg = "episode_phase"
        );

        let runtime = self.swe_runtime;
        let episode_id = episode.episode_id.clone();
        let policy = CommandPolicyConfig::default().with_mode(mode);
        // M2-2：优先经共享 L2 池（与 Gateway 同源）；无池时回退一次性 harness。
        let outcome = if let Some(pool) = self.swe_pool.clone() {
            let gold = if use_gold { Some(instance.patch.clone()) } else { None };
            let id = instance_id.clone();
            tokio::task::spawn_blocking(move || {
                pool.run_episode(&id, variant, policy, gold.as_deref())
            })
            .await
            .map_err(|e| Box::<dyn std::error::Error + Send + Sync>::from(format!("swe join error: {e}")))?
            .map_err(|err| {
                log_phase_error(&trace_id, &episode.episode_id, "swe_run", "ERR_SWE_RUN_FAILED", &*err);
                err
            })?
        } else {
            let opts = RunOptions {
                runtime,
                use_gold_patch: use_gold,
                keep_container: false,
                policy,
            };
            tokio::task::spawn_blocking(move || run_instance(&instance, &episode_id, &opts))
                .await
                .map_err(|e| Box::<dyn std::error::Error + Send + Sync>::from(format!("swe join error: {e}")))?
                .map_err(|err| {
                    log_phase_error(&trace_id, &episode.episode_id, "swe_run", "ERR_SWE_RUN_FAILED", &*err);
                    err
                })?
        };

        let reward = outcome.reward;
        let mut info = std::collections::HashMap::new();
        info.insert("instance_id".to_string(), instance_id.clone());
        info.insert("resolved".to_string(), outcome.resolved.to_string());
        info.insert("use_gold_patch".to_string(), use_gold.to_string());
        info.insert("benchmark_variant".to_string(), variant.as_str().to_string());
        if let Some(tr) = &outcome.artifact.test_results {
            let passed = tr.per_test.iter().filter(|(_, ok)| *ok).count();
            info.insert("tests_passed".to_string(), passed.to_string());
            info.insert("tests_total".to_string(), tr.per_test.len().to_string());
        }

        let step = StepRecord {
            step_index: 1,
            observation: Vec::new(),
            action: if use_gold { b"gold_patch".to_vec() } else { Vec::new() },
            reward,
            terminated: true,
            truncated: false,
            info,
            duration_ms: outcome.duration_ms as i64,
        };
        let trajectory = Trajectory {
            steps: vec![step.clone()],
            total_reward: reward,
            total_steps: 1,
        };
        let checksum = checksum_trajectory(&trajectory)?;
        let duration_ms = start.elapsed().as_millis() as u64;
        let result = EpisodeResult {
            episode_id: episode.episode_id.clone(),
            attempt_id: episode.attempt_id,
            status: "completed".to_string(),
            trajectory: Some(trajectory),
            summary: Some(crate::proto::v1::episode_result::Summary {
                total_reward: reward,
                total_steps: 1,
                total_duration_ms: duration_ms as i64,
                terminate_reason: "swe_evaluated".to_string(),
            }),
            error_code: None,
            error_message: String::new(),
            trajectory_checksum: checksum,
            integrity_verified: true,
        };
        let stream = StreamReport {
            episode_id: episode.episode_id.clone(),
            attempt_id: episode.attempt_id,
            current_step: 1,
            total_steps: 1,
            current_reward: reward,
            phase: "episode_complete".to_string(),
            last_step: Some(step),
            report_type: ReportType::Progress as i32,
            step_latency_ms: outcome.duration_ms as i64,
            model_latency_ms: 0,
            worker_active_episodes: ctx.active_episodes as i32,
            worker_capacity: ctx.worker_capacity as i32,
            correlation_id: episode.correlation_id.clone(),
            worker_id: ctx.worker_id.clone(),
            ..Default::default()
        };
        tracing::info!(
            trace_id = %trace_id,
            episode_id = %episode.episode_id,
            worker_id = %ctx.worker_id,
            instance_id = %instance_id,
            reward = reward,
            resolved = outcome.resolved,
            phase = "swe_complete",
            msg = "episode_phase"
        );
        Ok(ExecuteOutput {
            stream_reports: vec![stream],
            result,
            reward,
            duration_ms,
            env_step_duration_ms: outcome.duration_ms,
            model_callback_duration_ms: 0,
            warmup_hit: false,
        })
    }
}

fn log_phase_error(
    trace_id: &str,
    episode_id: &str,
    phase: &str,
    error_code: &str,
    err: &(dyn std::error::Error + Send + Sync),
) {
    tracing::error!(
        trace_id = %trace_id,
        episode_id = %episode_id,
        worker_id = "worker",
        phase = %phase,
        error_code = %error_code,
        error = %err,
        msg = "episode_phase_failed"
    );
}

fn checksum_trajectory(
    trajectory: &Trajectory,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let bytes = prost::Message::encode_to_vec(trajectory);
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    Ok(format!("{:x}", hasher.finalize()))
}
