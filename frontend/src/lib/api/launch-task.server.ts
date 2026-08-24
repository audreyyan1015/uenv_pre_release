import { spawn } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import process from "node:process";

export interface LaunchVerlPresetRequest {
  option_id: "verl";
  run_id?: string;
  model_path?: string;
  dataset_path?: string;
  rl_algorithm?: string;
  limit?: string;
  offset?: string;
  training_steps?: string;
  total_epochs?: string;
  train_batch_size?: string;
  ppo_mini_batch_size?: string;
  rollout_n?: string;
  temperature?: string;
  episode_max_steps?: string;
  max_prompt_length?: string;
  max_response_length?: string;
  parallel_mode?: string;
  save_freq?: string;
}

export interface LaunchVerlPresetResult {
  run_id: string;
  pid: number;
  script_path: string;
  log_file: string;
  launch_log_file: string;
  service_dir: string;
  checkpoint_dir: string;
  progress_path: string;
  server_path: string;
}

export interface StopVerlPresetRequest {
  run_id: string;
  pid: number;
}

export interface StopVerlPresetResult {
  run_id: string;
  pid: number;
  status: "signaled" | "not_found";
}

function formatTimestamp(date = new Date()) {
  const pad = (value: number) => String(value).padStart(2, "0");
  return [
    date.getFullYear(),
    pad(date.getMonth() + 1),
    pad(date.getDate()),
    "_",
    pad(date.getHours()),
    pad(date.getMinutes()),
    pad(date.getSeconds()),
  ].join("");
}

function resolveBridgeDir() {
  const candidates = [
    process.env.UENV_BRIDGE_DIR,
    "/data/ronghao/uenv/uenv-bridge",
    path.resolve(process.cwd(), "uenv-bridge"),
    path.resolve(process.cwd(), "../uenv-bridge"),
  ].filter((candidate): candidate is string => Boolean(candidate));

  for (const candidate of candidates) {
    const scriptPath = path.join(candidate, "scripts/train/presets/swe_smith_grpo_train.sh");
    if (fs.existsSync(scriptPath)) {
      return candidate;
    }
  }

  throw new Error(
    "未找到 uenv-bridge 目录。请在前端服务环境中设置 UENV_BRIDGE_DIR=/path/to/uenv-bridge。",
  );
}

function safeRunId(rawRunId?: string) {
  const runId = rawRunId?.trim();
  if (!runId) {
    return `verl_swesmith_grpo_train_${formatTimestamp()}`;
  }
  if (!/^[A-Za-z0-9_.-]+$/.test(runId)) {
    throw new Error("run_id 只能包含字母、数字、下划线、点和短横线。");
  }
  return runId;
}

function optionalTrim(value?: string) {
  const trimmed = value?.trim();
  return trimmed ? trimmed : undefined;
}

function intEnv(value: string | undefined, fallback: string, min = 0) {
  const normalized = optionalTrim(value) ?? fallback;
  if (!/^\d+$/.test(normalized)) {
    throw new Error(`参数必须是整数: ${normalized}`);
  }
  if (Number(normalized) < min) {
    throw new Error(`参数不能小于 ${min}: ${normalized}`);
  }
  return normalized;
}

function trainingStepsEnv(value: string | undefined) {
  const normalized = optionalTrim(value) ?? "null";
  if (normalized === "null") {
    return normalized;
  }
  if (!/^\d+$/.test(normalized) || Number(normalized) <= 0) {
    throw new Error("训练步数必须是正整数，或填写 null 表示按数据量和 epoch 自动计算。");
  }
  return normalized;
}

function floatEnv(value: string | undefined, fallback: string, min = 0) {
  const normalized = optionalTrim(value) ?? fallback;
  if (!/^(?:\d+(?:\.\d*)?|\.\d+)$/.test(normalized)) {
    throw new Error(`参数必须是数字: ${normalized}`);
  }
  if (Number(normalized) < min) {
    throw new Error(`参数不能小于 ${min}: ${normalized}`);
  }
  return normalized;
}

function setEnv(env: NodeJS.ProcessEnv, key: string, value: string | undefined) {
  if (value !== undefined) {
    env[key] = value;
  }
}

export function launchVerlPreset(input: LaunchVerlPresetRequest): LaunchVerlPresetResult {
  if (input.option_id !== "verl") {
    throw new Error("当前后端只支持启动 VeRL 训练任务。");
  }
  if ((input.rl_algorithm ?? "GRPO") !== "GRPO") {
    throw new Error("当前预设脚本只支持 GRPO。");
  }

  const bridgeDir = resolveBridgeDir();
  const scriptPath = path.join(bridgeDir, "scripts/train/presets/swe_smith_grpo_train.sh");
  const runId = safeRunId(input.run_id);
  const logRoot = process.env.UENV_LAUNCH_LOG_ROOT ?? path.join(bridgeDir, "temp/logs");
  const logFile = path.join(logRoot, "verl_layer4_agent_loop", `${runId}.log`);
  const launchLogFile = path.join(logRoot, "frontend_launch", `${runId}.log`);
  const serviceDir = path.join(logRoot, "layer4_distributed", runId);
  const checkpointRoot = process.env.CHECKPOINT_ROOT ?? path.join(bridgeDir, "checkpoints/uenv_grpo");
  const checkpointDir = path.join(checkpointRoot, runId);

  fs.mkdirSync(path.dirname(logFile), { recursive: true });
  fs.mkdirSync(path.dirname(launchLogFile), { recursive: true });
  fs.mkdirSync(serviceDir, { recursive: true });

  const env: NodeJS.ProcessEnv = { ...process.env };
  setEnv(env, "REPO_DIR", bridgeDir);
  setEnv(env, "RUN_ID", runId);
  setEnv(env, "UENV_TRAINING_RUN_ID", runId);
  setEnv(env, "LOG_ROOT", logRoot);
  setEnv(env, "LOG_FILE", logFile);
  setEnv(env, "LIMIT", intEnv(input.limit, "0", 0));
  setEnv(env, "OFFSET", intEnv(input.offset, "0", 0));
  setEnv(env, "TRAINING_STEPS", trainingStepsEnv(input.training_steps));
  setEnv(env, "TOTAL_EPOCHS", intEnv(input.total_epochs, "1", 1));
  setEnv(env, "TRAIN_BATCH_SIZE", intEnv(input.train_batch_size, "2", 1));
  setEnv(env, "PPO_MINI_BATCH_SIZE", intEnv(input.ppo_mini_batch_size, "2", 1));
  setEnv(env, "ROLLOUT_N", intEnv(input.rollout_n, "4", 1));
  setEnv(env, "ROLLOUT_TEMPERATURE", floatEnv(input.temperature, "1.0", 0));
  setEnv(env, "SWE_TRAJECTORY_MAX_STEPS", intEnv(input.episode_max_steps, "50", 1));
  setEnv(env, "UENV_EPISODE_MAX_STEPS_OVERRIDE", intEnv(input.episode_max_steps, "50", 1));
  setEnv(env, "MAX_PROMPT_LENGTH", intEnv(input.max_prompt_length, "8192", 1));
  setEnv(env, "DATA_MAX_RESPONSE_LENGTH", intEnv(input.max_response_length, "8192", 1));
  setEnv(env, "SAVE_FREQ", intEnv(input.save_freq, "5", 1));
  setEnv(env, "UENV_AGENT_LOOP_PARALLEL_MODE", optionalTrim(input.parallel_mode) ?? "sync");
  setEnv(env, "MODEL_PATH", optionalTrim(input.model_path));
  setEnv(env, "SWE_RAW_DATA_DIR", optionalTrim(input.dataset_path));

  const stdoutFd = fs.openSync(launchLogFile, "a");
  const stderrFd = fs.openSync(launchLogFile, "a");
  let child;
  try {
    child = spawn("bash", [scriptPath], {
      cwd: bridgeDir,
      detached: true,
      env,
      stdio: ["ignore", stdoutFd, stderrFd],
    });
  } finally {
    fs.closeSync(stdoutFd);
    fs.closeSync(stderrFd);
  }

  child.on("error", (error) => {
    fs.appendFile(
      launchLogFile,
      `[${new Date().toISOString()}] failed to spawn VeRL preset: ${String(error)}\n`,
      () => undefined,
    );
  });
  child.unref();

  if (!child.pid) {
    throw new Error("VeRL 训练进程启动失败，未获得 pid。");
  }

  return {
    run_id: runId,
    pid: child.pid,
    script_path: scriptPath,
    log_file: logFile,
    launch_log_file: launchLogFile,
    service_dir: serviceDir,
    checkpoint_dir: checkpointDir,
    progress_path: `/progress/${encodeURIComponent(runId)}`,
    server_path: `/server?run=${encodeURIComponent(runId)}`,
  };
}

export function stopVerlPreset(input: StopVerlPresetRequest): StopVerlPresetResult {
  if (!/^[A-Za-z0-9_.-]+$/.test(input.run_id)) {
    throw new Error("run_id 不合法。");
  }
  if (!Number.isInteger(input.pid) || input.pid <= 1) {
    throw new Error("pid 不合法。");
  }

  try {
    process.kill(-input.pid, "SIGTERM");
    return { run_id: input.run_id, pid: input.pid, status: "signaled" };
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ESRCH") {
      return { run_id: input.run_id, pid: input.pid, status: "not_found" };
    }
    throw error;
  }
}
