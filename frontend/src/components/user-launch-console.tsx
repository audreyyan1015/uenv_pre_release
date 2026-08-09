import { useEffect, useState } from "react";
import {
  Activity,
  ArrowUpRight,
  BarChart3,
  Download,
  FileUp,
  Gauge,
  Play,
  Radio,
  Square,
  Workflow,
} from "lucide-react";

type PrimarySection = "train" | "benchmark" | "observe";
type LaunchMode = "params" | "script";
type DemoRunState = "idle" | "running" | "stopping" | "stopped" | "completed";

type FieldConfig<T extends object> = {
  key: keyof T;
  label: string;
  type?: "text" | "number" | "select";
  options?: string[];
};

const trainTemplate = `#!/usr/bin/env bash
set -euo pipefail

export UENV_OBS_URL="http://8.130.75.157:8888/obs"
export UENV_ENABLE_THINKING=1
export MAX_PROMPT_LENGTH=24576
export DATA_MAX_RESPONSE_LENGTH=8192
export UENV_EPISODE_MAX_STEPS_OVERRIDE=2
export NGPUS_PER_NODE=8
export ROLLOUT_TP=1
export TRAIN_BATCH_SIZE=1
export ROLLOUT_N=8
export AGENT_NUM_WORKERS=8
export TRAINING_STEPS=50
export MODEL_PATH=/data/ronghao/models/modelscope/Qwen/Qwen3___6-35B-A3B
export DATA_DIR=/data/ronghao/uenv/uenv-bridge/data/benchmarks/swesmith_train_limit1000_offset0

bash /data/ronghao/uenv/uenv-bridge/scripts/train/presets/swe_smith_grpo_train.sh --limit 1000
`;

const benchmarkTemplate = `#!/usr/bin/env bash
set -euo pipefail

export UENV_OBS_URL="http://8.130.75.157:8888/obs"
export UENV_ADAPTER_CORE_ENDPOINT="8.130.75.157:8088"
export UENV_ROLLOUT_MODEL_ENDPOINT="http://127.0.0.1:18194/v1"
export UENV_ROLLOUT_MODEL_NAME="Qwen/Qwen3.6-35B-A3B"
export MAX_TOKENS=32768
export THINKING_TOKEN_BUDGET=16384
export PRESERVE_THINKING=false
export STRIP_REASONING=true

bash /data/ronghao/uenv/uenv-bridge/scripts/benchmark/run_pubmedqa_uenv_baseline.sh
`;

const navItems: Array<{
  id: PrimarySection;
  label: string;
  helper: string;
  icon: typeof Workflow;
}> = [
  {
    id: "train",
    label: "训练任务",
    helper: "VeRL + UEnv",
    icon: Workflow,
  },
  {
    id: "benchmark",
    label: "Benchmark 评测",
    helper: "五类基准",
    icon: BarChart3,
  },
  {
    id: "observe",
    label: "Server 可视化",
    helper: "Episode 进度",
    icon: Radio,
  },
];

const trainFields: FieldConfig<TrainParams>[] = [
  { key: "runId", label: "Run ID" },
  { key: "task", label: "任务类型", type: "select", options: ["SWE-smith", "SWE-bench-Pro"] },
  { key: "model", label: "基座模型" },
  { key: "datasetPath", label: "训练数据目录" },
  { key: "limit", label: "样本上限", type: "number" },
  { key: "trainingSteps", label: "训练步数", type: "number" },
  { key: "trainBatch", label: "训练 batch", type: "number" },
  { key: "rolloutN", label: "Rollout N", type: "number" },
  { key: "promptLength", label: "Prompt tokens", type: "number" },
  { key: "responseLength", label: "Response tokens", type: "number" },
  { key: "episodeMaxSteps", label: "Episode max steps", type: "number" },
  { key: "agentWorkers", label: "Agent workers", type: "number" },
  { key: "gpuCount", label: "GPU 数", type: "number" },
  { key: "rolloutTp", label: "Rollout TP", type: "number" },
  { key: "parallelMode", label: "并行模式", type: "select", options: ["sync", "fully_async"] },
  { key: "obsUrl", label: "Obs 地址" },
];

const benchmarkFields: FieldConfig<BenchmarkParams>[] = [
  { key: "runId", label: "Run ID" },
  {
    key: "benchmark",
    label: "Benchmark",
    type: "select",
    options: ["PubMedQA", "SciTab", "OlymMATH", "DSCodeBench", "SWE-bench-Pro"],
  },
  { key: "model", label: "评测模型" },
  { key: "limit", label: "样本上限", type: "number" },
  { key: "maxTokens", label: "Max tokens", type: "number" },
  { key: "thinkingBudget", label: "Thinking budget", type: "number" },
  { key: "promptStyle", label: "Prompt", type: "select", options: ["official", "uenv"] },
  { key: "adapterEndpoint", label: "Adapter endpoint" },
  { key: "modelEndpoint", label: "Model endpoint" },
  { key: "outputDir", label: "输出目录" },
  { key: "obsUrl", label: "Obs 地址" },
];

interface TrainParams {
  runId: string;
  task: string;
  model: string;
  datasetPath: string;
  limit: string;
  trainingSteps: string;
  trainBatch: string;
  rolloutN: string;
  promptLength: string;
  responseLength: string;
  episodeMaxSteps: string;
  agentWorkers: string;
  gpuCount: string;
  rolloutTp: string;
  parallelMode: string;
  obsUrl: string;
}

interface BenchmarkParams {
  runId: string;
  benchmark: string;
  model: string;
  limit: string;
  maxTokens: string;
  thinkingBudget: string;
  promptStyle: string;
  adapterEndpoint: string;
  modelEndpoint: string;
  outputDir: string;
  obsUrl: string;
}

function dataHref(text: string) {
  return `data:text/x-shellscript;charset=utf-8,${encodeURIComponent(text)}`;
}

function updateValue<T extends object>(
  setter: React.Dispatch<React.SetStateAction<T>>,
  key: keyof T,
  value: string,
) {
  setter((current) => ({ ...current, [key]: value }));
}

function demoEvent(message: string) {
  return `${new Date().toLocaleTimeString("zh-CN", { hour12: false })}  ${message}`;
}

function Field<T extends object>({
  field,
  value,
  onChange,
}: {
  field: FieldConfig<T>;
  value: string;
  onChange: (value: string) => void;
}) {
  return (
    <label className="grid gap-1.5 text-sm">
      <span className="font-medium text-slate-700">{field.label}</span>
      {field.type === "select" ? (
        <select
          value={value}
          onChange={(event) => onChange(event.target.value)}
          className="h-10 rounded-md border border-slate-200 bg-white px-3 text-sm text-slate-900 outline-none transition focus:border-blue-400 focus:ring-2 focus:ring-blue-100"
        >
          {(field.options ?? []).map((option) => (
            <option key={option} value={option}>
              {option}
            </option>
          ))}
        </select>
      ) : (
        <input
          value={value}
          type={field.type ?? "text"}
          onChange={(event) => onChange(event.target.value)}
          className="h-10 rounded-md border border-slate-200 bg-white px-3 text-sm text-slate-900 outline-none transition focus:border-blue-400 focus:ring-2 focus:ring-blue-100"
        />
      )}
    </label>
  );
}

function ModeTabs({ mode, onChange }: { mode: LaunchMode; onChange: (mode: LaunchMode) => void }) {
  return (
    <div className="inline-flex h-10 rounded-md border border-slate-200 bg-white p-1 text-sm font-medium">
      {[
        ["params", "按参数配置"],
        ["script", "上传脚本"],
      ].map(([id, label]) => (
        <button
          key={id}
          type="button"
          aria-pressed={mode === id}
          onClick={() => onChange(id as LaunchMode)}
          className={`rounded px-4 transition ${
            mode === id
              ? "bg-blue-600 text-white shadow-sm"
              : "text-slate-500 hover:bg-slate-100 hover:text-slate-800"
          }`}
        >
          {label}
        </button>
      ))}
    </div>
  );
}

function ScriptUploadPanel({
  kind,
  template,
  script,
  onScriptChange,
}: {
  kind: "训练" | "评测";
  template: string;
  script: string;
  onScriptChange: (value: string) => void;
}) {
  return (
    <div className="grid gap-4">
      <div className="grid gap-3 rounded-lg border border-slate-200 bg-white p-4">
        <div className="flex flex-wrap items-center justify-between gap-3">
          <div className="flex items-center gap-2 text-sm font-semibold text-slate-800">
            <FileUp className="h-4 w-4 text-blue-600" />
            {kind}脚本
          </div>
          <a
            href={dataHref(template)}
            download={kind === "训练" ? "uenv-train-template.sh" : "uenv-benchmark-template.sh"}
            className="inline-flex h-9 items-center gap-2 rounded-md border border-slate-200 bg-white px-3 text-xs font-medium text-slate-700 transition hover:border-blue-200 hover:bg-blue-50 hover:text-blue-700"
          >
            <Download className="h-3.5 w-3.5" />
            下载模板
          </a>
        </div>
        <input
          type="file"
          accept=".sh,.bash,.yaml,.yml,.json,.txt"
          className="h-10 rounded-md border border-dashed border-slate-300 bg-slate-50 px-3 py-2 text-sm text-slate-600 file:mr-3 file:rounded file:border-0 file:bg-blue-600 file:px-3 file:py-1 file:text-xs file:font-semibold file:text-white"
        />
        <textarea
          value={script}
          onChange={(event) => onScriptChange(event.target.value)}
          rows={14}
          spellCheck={false}
          className="min-h-[320px] resize-y rounded-md border border-slate-200 bg-slate-950 p-4 font-mono text-xs leading-5 text-slate-100 outline-none transition focus:border-blue-400 focus:ring-2 focus:ring-blue-100"
        />
      </div>
      <div className="rounded-lg border border-slate-200 bg-slate-50 p-4">
        <p className="text-sm font-semibold text-slate-800">覆盖参数</p>
        <textarea
          rows={5}
          spellCheck={false}
          placeholder="KEY=value"
          className="mt-3 w-full resize-y rounded-md border border-slate-200 bg-white p-3 font-mono text-xs leading-5 outline-none transition focus:border-blue-400 focus:ring-2 focus:ring-blue-100"
        />
      </div>
    </div>
  );
}

function LaunchPreview({
  runId,
  demoLabel,
  demoState,
  demoProgress,
  demoEvents,
  onStartDemo,
  onStopDemo,
}: {
  runId: string;
  demoLabel: "训练" | "评测";
  demoState: DemoRunState;
  demoProgress: number;
  demoEvents: string[];
  onStartDemo: () => void;
  onStopDemo: () => void;
}) {
  const serverHref = `/server${runId ? `?run=${encodeURIComponent(runId)}` : ""}`;
  const opsHref = `/ops${runId ? `?run=${encodeURIComponent(runId)}` : ""}`;
  const systemHref = `/system${runId ? `?run=${encodeURIComponent(runId)}` : ""}`;
  const demoMeta: Record<DemoRunState, { label: string; className: string; dot: string }> = {
    idle: {
      label: "待启动",
      className: "border-slate-200 bg-slate-50 text-slate-600",
      dot: "bg-slate-400",
    },
    running: {
      label: "运行中",
      className: "border-blue-200 bg-blue-50 text-blue-700",
      dot: "bg-blue-500",
    },
    stopping: {
      label: "终止中",
      className: "border-amber-200 bg-amber-50 text-amber-700",
      dot: "bg-amber-500",
    },
    stopped: {
      label: "已终止",
      className: "border-slate-200 bg-slate-100 text-slate-700",
      dot: "bg-slate-500",
    },
    completed: {
      label: "已完成",
      className: "border-emerald-200 bg-emerald-50 text-emerald-700",
      dot: "bg-emerald-500",
    },
  };
  const currentDemo = demoMeta[demoState];

  return (
    <section className="mt-5 rounded-lg border border-slate-200 bg-slate-50 p-4">
      <div className="grid gap-3 xl:grid-cols-[180px_minmax(180px,1fr)_260px_240px] xl:items-center">
        <div className="flex items-center justify-between gap-3 xl:block">
          <p className="text-sm font-semibold text-slate-900">任务状态</p>
          <span
            className={`inline-flex h-7 items-center gap-1.5 rounded-md border px-2 text-xs font-medium xl:mt-2 ${currentDemo.className}`}
          >
            <span className={`h-2 w-2 rounded-full ${currentDemo.dot}`} />
            {currentDemo.label}
          </span>
        </div>
        <div className="grid gap-2">
          <div className="flex items-center justify-between text-xs text-slate-500">
            <span>进度</span>
            <span className="tabular-nums">{Math.round(demoProgress)}%</span>
          </div>
          <div className="h-2 overflow-hidden rounded-full bg-white">
            <div
              className="h-full rounded-full bg-blue-600 transition-all"
              style={{ width: `${Math.min(100, Math.max(0, demoProgress))}%` }}
            />
          </div>
        </div>
        <div className="grid grid-cols-2 gap-2">
          <button
            type="button"
            disabled={demoState === "running" || demoState === "stopping"}
            onClick={onStartDemo}
            className="inline-flex h-10 items-center justify-center gap-2 rounded-md bg-blue-600 px-3 text-sm font-semibold text-white transition hover:bg-blue-700 disabled:cursor-not-allowed disabled:bg-slate-300"
          >
            <Play className="h-4 w-4" />
            启动{demoLabel}
          </button>
          <button
            type="button"
            disabled={demoState !== "running"}
            onClick={onStopDemo}
            className="inline-flex h-10 items-center justify-center gap-2 rounded-md border border-rose-200 bg-white px-3 text-sm font-semibold text-rose-700 transition hover:bg-rose-50 disabled:cursor-not-allowed disabled:border-slate-200 disabled:text-slate-300"
          >
            <Square className="h-4 w-4" />
            终止{demoLabel}
          </button>
        </div>
        <div className="grid grid-cols-3 gap-2">
          <a
            href={systemHref}
            className="inline-flex h-10 items-center justify-center gap-2 rounded-md border border-slate-200 bg-white px-3 text-sm font-semibold text-slate-700 transition hover:border-blue-200 hover:bg-blue-50 hover:text-blue-700"
          >
            <Workflow className="h-4 w-4" />
            系统拓扑
          </a>
          <a
            href={serverHref}
            className="inline-flex h-10 items-center justify-center gap-2 rounded-md border border-slate-200 bg-white px-3 text-sm font-semibold text-slate-700 transition hover:border-blue-200 hover:bg-blue-50 hover:text-blue-700"
          >
            <Activity className="h-4 w-4" />
            打开进度
          </a>
          <a
            href={opsHref}
            className="inline-flex h-10 items-center justify-center gap-2 rounded-md border border-slate-200 bg-white px-3 text-sm font-semibold text-slate-700 transition hover:border-blue-200 hover:bg-blue-50 hover:text-blue-700"
          >
            <Gauge className="h-4 w-4" />
            打开观测台
          </a>
        </div>
      </div>
      <div className="mt-3 max-h-20 overflow-auto rounded-md border border-slate-200 bg-white p-3 font-mono text-[11px] leading-5 text-slate-600">
        {demoEvents.map((event) => (
          <div key={event}>{event}</div>
        ))}
      </div>
    </section>
  );
}

function TrainForm({
  params,
  onChange,
}: {
  params: TrainParams;
  onChange: (key: keyof TrainParams, value: string) => void;
}) {
  return (
    <div className="grid gap-4 md:grid-cols-2">
      {trainFields.map((field) => (
        <Field
          key={String(field.key)}
          field={field}
          value={params[field.key]}
          onChange={(value) => onChange(field.key, value)}
        />
      ))}
    </div>
  );
}

function BenchmarkForm({
  params,
  onChange,
}: {
  params: BenchmarkParams;
  onChange: (key: keyof BenchmarkParams, value: string) => void;
}) {
  return (
    <div className="grid gap-4 md:grid-cols-2">
      {benchmarkFields.map((field) => (
        <Field
          key={String(field.key)}
          field={field}
          value={params[field.key]}
          onChange={(value) => onChange(field.key, value)}
        />
      ))}
    </div>
  );
}

function ObservePanel({
  runId,
  onRunIdChange,
}: {
  runId: string;
  onRunIdChange: (value: string) => void;
}) {
  const serverHref = `/server${runId ? `?run=${encodeURIComponent(runId)}` : ""}`;
  const opsHref = `/ops${runId ? `?run=${encodeURIComponent(runId)}` : ""}`;
  const systemHref = `/system${runId ? `?run=${encodeURIComponent(runId)}` : ""}`;

  return (
    <div className="grid gap-5">
      <div className="grid gap-4 rounded-lg border border-slate-200 bg-white p-5">
        <div className="flex items-center gap-2 text-sm font-semibold text-slate-800">
          <Radio className="h-4 w-4 text-blue-600" />
          Server 可视化
        </div>
        <label className="grid gap-1.5 text-sm">
          <span className="font-medium text-slate-700">Run ID</span>
          <input
            value={runId}
            onChange={(event) => onRunIdChange(event.target.value)}
            className="h-10 rounded-md border border-slate-200 bg-white px-3 text-sm text-slate-900 outline-none transition focus:border-blue-400 focus:ring-2 focus:ring-blue-100"
          />
        </label>
        <div className="grid gap-3 sm:grid-cols-3">
          <a
            href={systemHref}
            className="flex h-24 items-center justify-between rounded-lg border border-slate-200 bg-slate-50 px-4 transition hover:border-blue-200 hover:bg-blue-50"
          >
            <span>
              <span className="block text-sm font-semibold text-slate-900">系统拓扑</span>
              <span className="mt-1 block text-xs text-slate-500">
                adapter / server / worker / hub
              </span>
            </span>
            <ArrowUpRight className="h-4 w-4 text-slate-400" />
          </a>
          <a
            href={serverHref}
            className="flex h-24 items-center justify-between rounded-lg border border-slate-200 bg-slate-50 px-4 transition hover:border-blue-200 hover:bg-blue-50"
          >
            <span>
              <span className="block text-sm font-semibold text-slate-900">Episode 进度</span>
              <span className="mt-1 block text-xs text-slate-500">server.tsx</span>
            </span>
            <ArrowUpRight className="h-4 w-4 text-slate-400" />
          </a>
          <a
            href={opsHref}
            className="flex h-24 items-center justify-between rounded-lg border border-slate-200 bg-slate-50 px-4 transition hover:border-blue-200 hover:bg-blue-50"
          >
            <span>
              <span className="block text-sm font-semibold text-slate-900">技术观测台</span>
              <span className="mt-1 block text-xs text-slate-500">workflow / event stream</span>
            </span>
            <ArrowUpRight className="h-4 w-4 text-slate-400" />
          </a>
        </div>
      </div>
    </div>
  );
}

export function UserLaunchConsole({ initialRunId = null }: { initialRunId?: string | null }) {
  const [section, setSection] = useState<PrimarySection>("train");
  const [mode, setMode] = useState<LaunchMode>("params");
  const [observeRunId, setObserveRunId] = useState(initialRunId ?? "");
  const [trainScript, setTrainScript] = useState(trainTemplate);
  const [benchmarkScript, setBenchmarkScript] = useState(benchmarkTemplate);
  const [demoState, setDemoState] = useState<DemoRunState>("idle");
  const [demoProgress, setDemoProgress] = useState(0);
  const [demoEvents, setDemoEvents] = useState<string[]>([demoEvent("等待启动")]);
  const [trainParams, setTrainParams] = useState<TrainParams>({
    runId: initialRunId ?? "swe-smith-grpo-train",
    task: "SWE-smith",
    model: "/data/ronghao/models/modelscope/Qwen/Qwen3___6-35B-A3B",
    datasetPath: "/data/ronghao/uenv/uenv-bridge/data/benchmarks/swesmith_train_limit1000_offset0",
    limit: "1000",
    trainingSteps: "50",
    trainBatch: "1",
    rolloutN: "8",
    promptLength: "24576",
    responseLength: "8192",
    episodeMaxSteps: "2",
    agentWorkers: "8",
    gpuCount: "8",
    rolloutTp: "1",
    parallelMode: "sync",
    obsUrl: "http://8.130.75.157:8888/obs",
  });
  const [benchmarkParams, setBenchmarkParams] = useState<BenchmarkParams>({
    runId: initialRunId ?? "pubmedqa-uenv-eval",
    benchmark: "PubMedQA",
    model: "Qwen/Qwen3.6-35B-A3B",
    limit: "full",
    maxTokens: "32768",
    thinkingBudget: "16384",
    promptStyle: "official",
    adapterEndpoint: "http://127.0.0.1:50051",
    modelEndpoint: "http://127.0.0.1:18194/v1",
    outputDir: "/data/ronghao/uenv/uenv-bridge/temp/benchmarks",
    obsUrl: "http://8.130.75.157:8888/obs",
  });

  const title =
    section === "train" ? "训练任务" : section === "benchmark" ? "Benchmark 评测" : "Server 可视化";
  const subtitle =
    section === "train"
      ? "配置 VeRL + UEnv 训练入口"
      : section === "benchmark"
        ? "配置五类 benchmark 评测入口"
        : "查看 Server 侧 Episode 与 Worker 进度";

  useEffect(() => {
    if (demoState !== "running") return undefined;

    const timer = window.setInterval(() => {
      setDemoProgress((current) => {
        const next = Math.min(100, current + 7);
        if (next >= 100) {
          setDemoState("completed");
          setDemoEvents((events) => [demoEvent("任务完成"), ...events].slice(0, 8));
        }
        return next;
      });
    }, 1200);

    return () => window.clearInterval(timer);
  }, [demoState]);

  useEffect(() => {
    if (demoState !== "stopping") return undefined;
    const timer = window.setTimeout(() => {
      setDemoState("stopped");
      setDemoEvents((events) => [demoEvent("任务已终止"), ...events].slice(0, 8));
    }, 800);
    return () => window.clearTimeout(timer);
  }, [demoState]);

  function startDemo(label: "训练" | "评测") {
    setDemoState("running");
    setDemoProgress(4);
    setDemoEvents([demoEvent(`启动${label}`), demoEvent("创建本地 demo run")]);
  }

  function stopDemo(label: "训练" | "评测") {
    setDemoState("stopping");
    setDemoEvents((events) => [demoEvent(`终止${label}`), ...events].slice(0, 8));
  }

  const showPreview = section !== "observe";
  const previewRunId = section === "train" ? trainParams.runId : benchmarkParams.runId;

  return (
    <main className="min-h-screen bg-slate-100 p-4 text-slate-900 lg:p-6">
      <div className="mx-auto grid min-h-[calc(100vh-2rem)] max-w-[1600px] overflow-hidden rounded-lg border border-slate-200 bg-white shadow-sm lg:min-h-[calc(100vh-3rem)]">
        <header className="flex h-auto flex-col gap-4 border-b border-slate-200 bg-slate-50 px-4 py-4 sm:flex-row sm:items-center sm:justify-between lg:px-6">
          <div className="flex min-w-0 items-center gap-3">
            <div className="flex h-10 w-10 shrink-0 items-center justify-center rounded-md bg-blue-600 font-semibold text-white">
              UE
            </div>
            <div className="min-w-0">
              <h1 className="truncate text-lg font-semibold tracking-normal text-slate-950">
                UEnv 训练与评测控制台
              </h1>
              <p className="mt-0.5 truncate text-xs text-slate-500">{subtitle}</p>
            </div>
          </div>
          <div className="flex flex-wrap items-center gap-2 text-xs text-slate-500">
            <span className="inline-flex h-8 items-center gap-2 rounded-md border border-blue-200 bg-blue-50 px-3 text-blue-700">
              <span className="h-2 w-2 rounded-full bg-blue-500" />
              本地 Demo
            </span>
          </div>
        </header>

        <div className="border-b border-slate-200 bg-white px-4 py-3 lg:px-6">
          <div className="flex flex-wrap gap-2">
            {navItems.map((item) => {
              const Icon = item.icon;
              const active = section === item.id;
              return (
                <button
                  key={item.id}
                  type="button"
                  aria-pressed={active}
                  onClick={() => {
                    setSection(item.id);
                    if (item.id !== "observe") setMode("params");
                  }}
                  className={`inline-flex h-11 items-center gap-2 rounded-md border px-4 text-left transition ${
                    active
                      ? "border-blue-200 bg-blue-50 text-blue-800"
                      : "border-slate-200 bg-white text-slate-600 hover:bg-slate-50"
                  }`}
                >
                  <Icon className="h-4 w-4 shrink-0" />
                  <span className="min-w-0">
                    <span className="block truncate text-sm font-semibold">{item.label}</span>
                    <span className="block truncate text-[11px] text-slate-500">{item.helper}</span>
                  </span>
                </button>
              );
            })}
          </div>
        </div>

        <div className="grid min-h-0">
          <section className="min-w-0 overflow-auto bg-white p-4 lg:p-6">
            <div className="mb-5 flex flex-col gap-3 md:flex-row md:items-center md:justify-between">
              <div>
                <p className="text-sm font-medium text-blue-700">UEnv Adapter</p>
                <h2 className="mt-1 text-2xl font-semibold tracking-normal">{title}</h2>
              </div>
              {section !== "observe" && <ModeTabs mode={mode} onChange={setMode} />}
            </div>

            {section === "train" && mode === "params" && (
              <TrainForm
                params={trainParams}
                onChange={(key, value) => updateValue(setTrainParams, key, value)}
              />
            )}

            {section === "train" && mode === "script" && (
              <ScriptUploadPanel
                kind="训练"
                template={trainTemplate}
                script={trainScript}
                onScriptChange={setTrainScript}
              />
            )}

            {section === "benchmark" && mode === "params" && (
              <BenchmarkForm
                params={benchmarkParams}
                onChange={(key, value) => updateValue(setBenchmarkParams, key, value)}
              />
            )}

            {section === "benchmark" && mode === "script" && (
              <ScriptUploadPanel
                kind="评测"
                template={benchmarkTemplate}
                script={benchmarkScript}
                onScriptChange={setBenchmarkScript}
              />
            )}

            {section === "observe" && (
              <ObservePanel runId={observeRunId} onRunIdChange={setObserveRunId} />
            )}

            {showPreview && (
              <LaunchPreview
                runId={previewRunId}
                demoLabel={section === "train" ? "训练" : "评测"}
                demoState={demoState}
                demoProgress={demoProgress}
                demoEvents={demoEvents}
                onStartDemo={() => startDemo(section === "train" ? "训练" : "评测")}
                onStopDemo={() => stopDemo(section === "train" ? "训练" : "评测")}
              />
            )}
          </section>
        </div>
      </div>
    </main>
  );
}
