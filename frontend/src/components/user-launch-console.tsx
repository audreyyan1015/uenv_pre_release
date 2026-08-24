import { useEffect, useState } from "react";
import type { LucideIcon } from "lucide-react";
import {
  Activity,
  ArrowLeft,
  BarChart3,
  Bot,
  BrainCircuit,
  ChevronDown,
  ChevronRight,
  ClipboardCheck,
  Code2,
  Database,
  FileStack,
  FlaskConical,
  Gauge,
  GitBranch,
  Layers3,
  Play,
  RotateCcw,
  Square,
  Table2,
  Workflow,
} from "lucide-react";
import {
  AggregationClient,
  getAggregationConfig,
  type RunSummaryPage,
  type RunSummaryStatusCounts,
  type RunSummary,
  type RunTimelineItem,
} from "@/lib/api/aggregation-client";
import { launchVerlTraining, stopVerlTraining } from "@/lib/api/launch-task.functions";
import type { ChainState, NodeStatus, WorkflowStage } from "@/lib/types/chain-state";

type CategoryId = "train" | "benchmark" | "trajectory";
type DemoRunState = "idle" | "running" | "stopping" | "stopped" | "completed" | "failed";
type FieldType = "text" | "number" | "select" | "textarea";

interface CategoryConfig {
  id: CategoryId;
  title: string;
  subtitle: string;
  icon: LucideIcon;
  tone: string;
  comingSoonTitle: string;
  comingSoonDescription: string;
}

interface FieldConfig {
  key: string;
  label: string;
  type?: FieldType;
  options?: string[];
  wide?: boolean;
  placeholder?: string;
}

interface LaunchOption {
  id: string;
  category: CategoryId;
  title: string;
  description: string;
  icon: LucideIcon;
  fields: FieldConfig[];
  defaults: Record<string, string>;
}

interface ActiveBackendRun {
  run_id: string;
  pid: number;
  log_file: string;
  launch_log_file: string;
  service_dir: string;
  checkpoint_dir: string;
  progress_path: string;
  server_path: string;
}

const categories: CategoryConfig[] = [
  {
    id: "train",
    title: "大模型后训练",
    subtitle: "用训练数据和环境反馈持续优化模型能力",
    icon: BrainCircuit,
    tone: "border-[#0070F3]/20 bg-[#0070F3]/5 text-[#0070F3]",
    comingSoonTitle: "其他训练框架",
    comingSoonDescription: "更多后训练框架正在支持中",
  },
  {
    id: "benchmark",
    title: "评测",
    subtitle: "用标准任务衡量模型在不同能力维度上的表现",
    icon: ClipboardCheck,
    tone: "border-[#0070F3]/20 bg-[#0070F3]/5 text-[#0070F3]",
    comingSoonTitle: "其他评测任务",
    comingSoonDescription: "更多 benchmark 正在支持中",
  },
  {
    id: "trajectory",
    title: "轨迹采集",
    subtitle: "记录模型与环境交互过程，支持回放、分析和复用",
    icon: GitBranch,
    tone: "border-[#0070F3]/20 bg-[#0070F3]/5 text-[#0070F3]",
    comingSoonTitle: "其他轨迹类型",
    comingSoonDescription: "更多采集场景正在支持中",
  },
];

const verlFields: FieldConfig[] = [
  { key: "run_id", label: "run_id", placeholder: "留空自动生成" },
  { key: "model_path", label: "基座模型" },
  { key: "dataset_path", label: "训练数据" },
  { key: "rl_algorithm", label: "强化学习算法", type: "select", options: ["GRPO"] },
  { key: "limit", label: "样本数量", type: "number" },
  { key: "offset", label: "样本偏移", type: "number" },
  { key: "training_steps", label: "训练步数", placeholder: "null 表示自动计算" },
  { key: "total_epochs", label: "训练轮数", type: "number" },
  { key: "train_batch_size", label: "训练批大小", type: "number" },
  { key: "ppo_mini_batch_size", label: "PPO 小批大小", type: "number" },
  { key: "rollout_n", label: "采样数", type: "number" },
  { key: "temperature", label: "温度系数", type: "number" },
  { key: "episode_max_steps", label: "最大交互步数", type: "number" },
  { key: "max_prompt_length", label: "最大输入长度", type: "number" },
  { key: "max_response_length", label: "最大输出长度", type: "number" },
  { key: "parallel_mode", label: "并行模式", type: "select", options: ["sync", "fully_async"] },
  { key: "save_freq", label: "保存间隔（步）", type: "number" },
];

const frameworkFields: FieldConfig[] = [
  { key: "run_id", label: "run_id" },
  { key: "model_path", label: "基座模型" },
  { key: "dataset_path", label: "训练数据" },
  { key: "rl_algorithm", label: "强化学习算法", type: "select", options: ["GRPO", "PPO", "DAPO"] },
  { key: "training_steps", label: "训练步数", type: "number" },
  { key: "train_batch_size", label: "训练批大小", type: "number" },
  { key: "rollout_n", label: "采样数", type: "number" },
  { key: "temperature", label: "温度系数", type: "number" },
  { key: "max_prompt_length", label: "最大输入长度", type: "number" },
  { key: "max_response_length", label: "最大输出长度", type: "number" },
  { key: "parallel_mode", label: "并行模式", type: "select", options: ["sync", "fully_async"] },
  { key: "save_freq", label: "保存间隔（步）", type: "number" },
];

const benchmarkFields: FieldConfig[] = [
  { key: "run_id", label: "run_id" },
  { key: "model_name", label: "模型名称" },
  { key: "model_endpoint", label: "模型接口" },
  { key: "sample_limit", label: "样本数量" },
  { key: "max_tokens", label: "最大生成长度", type: "number" },
  { key: "thinking_budget", label: "思考预算", type: "number" },
  { key: "prompt_style", label: "提示模板", type: "select", options: ["official", "uenv", "cot"] },
  { key: "output_dir", label: "输出目录" },
  { key: "notes", label: "备注", type: "textarea", wide: true },
];

const trajectoryFields: FieldConfig[] = [
  { key: "run_id", label: "run_id" },
  { key: "source", label: "任务来源" },
  { key: "model_endpoint", label: "模型接口" },
  { key: "sample_limit", label: "采集样本数", type: "number" },
  { key: "episode_max_steps", label: "最大交互步数", type: "number" },
  { key: "max_response_length", label: "最大输出长度", type: "number" },
  { key: "save_dir", label: "轨迹保存目录" },
  { key: "trace_format", label: "轨迹格式", type: "select", options: ["jsonl", "parquet"] },
  { key: "capture_tool_calls", label: "记录工具调用", type: "select", options: ["开启", "关闭"] },
  { key: "notes", label: "备注", type: "textarea", wide: true },
];

const launchOptions: LaunchOption[] = [
  {
    id: "verl",
    category: "train",
    title: "VeRL",
    description: "VeRL是由字节跳动Seed团队发起的LLM强化学习训练框架",
    icon: Workflow,
    fields: verlFields,
    defaults: {
      run_id: "",
      model_path: "/data/ronghao/models/modelscope/Qwen/Qwen3___6-35B-A3B",
      dataset_path: "/data/ronghao/uenv/uenv-bridge/data/benchmarks/swesmith/raw/data",
      rl_algorithm: "GRPO",
      limit: "0",
      offset: "0",
      training_steps: "null",
      total_epochs: "1",
      train_batch_size: "2",
      ppo_mini_batch_size: "2",
      rollout_n: "4",
      temperature: "1.0",
      episode_max_steps: "50",
      max_prompt_length: "8192",
      max_response_length: "8192",
      parallel_mode: "sync",
      save_freq: "5",
    },
  },
  {
    id: "roll",
    category: "train",
    title: "ROLL",
    description: "ROLL是由阿里巴巴开源的强化学习训练框架",
    icon: Layers3,
    fields: frameworkFields,
    defaults: {
      run_id: "uenv_roll_train",
      model_path: "/data/ronghao/models/modelscope/Qwen/Qwen3___6-35B-A3B",
      dataset_path: "/data/ronghao/uenv/uenv-bridge/data/train",
      rl_algorithm: "GRPO",
      training_steps: "20",
      train_batch_size: "1",
      rollout_n: "4",
      temperature: "1.0",
      max_prompt_length: "24576",
      max_response_length: "8192",
      parallel_mode: "sync",
      save_freq: "50",
    },
  },
  {
    id: "nexrl",
    category: "train",
    title: "NexRL",
    description: "NexRL是由Nex-AGI开源的超松耦合LLM后训练框架",
    icon: Bot,
    fields: [
      ...frameworkFields,
      { key: "rollout_workers", label: "采样进程数", type: "number" },
      { key: "inference_service", label: "推理服务地址" },
    ],
    defaults: {
      run_id: "uenv_nexrl_train",
      model_path: "/data/ronghao/models/modelscope/Qwen/Qwen3___6-35B-A3B",
      dataset_path: "/data/ronghao/uenv/uenv-bridge/data/train",
      rl_algorithm: "GRPO",
      training_steps: "20",
      train_batch_size: "1",
      rollout_n: "4",
      temperature: "1.0",
      max_prompt_length: "24576",
      max_response_length: "8192",
      parallel_mode: "sync",
      save_freq: "50",
      rollout_workers: "4",
      inference_service: "http://127.0.0.1:18194/v1",
    },
  },
  {
    id: "pubmedqa",
    category: "benchmark",
    title: "PubMedQA",
    description: "面向医学 QA 的闭集选择题评测",
    icon: FlaskConical,
    fields: benchmarkFields,
    defaults: {
      run_id: "pubmedqa_uenv_eval",
      model_name: "Qwen/Qwen3.6-35B-A3B",
      model_endpoint: "http://127.0.0.1:18194/v1",
      sample_limit: "full",
      max_tokens: "32768",
      thinking_budget: "16384",
      prompt_style: "official",
      output_dir: "/data/ronghao/uenv/uenv-bridge/temp/benchmarks/pubmedqa",
      notes: "输出 yes / no / maybe。",
    },
  },
  {
    id: "scitab",
    category: "benchmark",
    title: "SciTab",
    description: "面向科学表格理解与声明判断的评测",
    icon: Table2,
    fields: benchmarkFields,
    defaults: {
      run_id: "scitab_uenv_eval",
      model_name: "Qwen/Qwen3.6-35B-A3B",
      model_endpoint: "http://127.0.0.1:18194/v1",
      sample_limit: "full",
      max_tokens: "32768",
      thinking_budget: "16384",
      prompt_style: "official",
      output_dir: "/data/ronghao/uenv/uenv-bridge/temp/benchmarks/scitab",
      notes: "输出 entailment / contradiction / neutral。",
    },
  },
  {
    id: "dscodebench",
    category: "benchmark",
    title: "DSCodeBench",
    description: "面向代码生成、数据处理和结果验证的评测",
    icon: Code2,
    fields: benchmarkFields,
    defaults: {
      run_id: "dscodebench_uenv_eval",
      model_name: "Qwen/Qwen3.6-35B-A3B",
      model_endpoint: "http://127.0.0.1:18194/v1",
      sample_limit: "full",
      max_tokens: "32768",
      thinking_budget: "16384",
      prompt_style: "uenv",
      output_dir: "/data/ronghao/uenv/uenv-bridge/temp/benchmarks/dscodebench",
      notes: "保留代码执行结果和错误信息。",
    },
  },
  {
    id: "swe",
    category: "benchmark",
    title: "SWE",
    description: "面向 SWE-bench 的环境执行评测",
    icon: Database,
    fields: benchmarkFields,
    defaults: {
      run_id: "swe_uenv_eval",
      model_name: "Qwen/Qwen3.6-35B-A3B",
      model_endpoint: "http://127.0.0.1:18194/v1",
      sample_limit: "full",
      max_tokens: "32768",
      thinking_budget: "16384",
      prompt_style: "uenv",
      output_dir: "/data/ronghao/uenv/uenv-bridge/temp/benchmarks/swe",
      notes: "记录 patch、测试日志和 resolved。",
    },
  },
  {
    id: "olympic-math",
    category: "benchmark",
    title: "Olympic Math",
    description: "面向奥林匹克数学题的长思维链评测",
    icon: BarChart3,
    fields: benchmarkFields,
    defaults: {
      run_id: "olympic_math_uenv_eval",
      model_name: "Qwen/Qwen3.6-35B-A3B",
      model_endpoint: "http://127.0.0.1:18194/v1",
      sample_limit: "full",
      max_tokens: "32768",
      thinking_budget: "16384",
      prompt_style: "cot",
      output_dir: "/data/ronghao/uenv/uenv-bridge/temp/benchmarks/olympic_math",
      notes: "最终答案要求放入 boxed{}。",
    },
  },
  {
    id: "swe-trajectory",
    category: "trajectory",
    title: "SWE 轨迹采集",
    description: "采集 Agent 在代码修复任务中的完整操作链，涵盖文件编辑、测试执行及最终补丁生成",
    icon: FileStack,
    fields: trajectoryFields,
    defaults: {
      run_id: "swe_trajectory_collect",
      source: "SWE-smith",
      model_endpoint: "http://127.0.0.1:18194/v1",
      sample_limit: "100",
      episode_max_steps: "50",
      max_response_length: "8192",
      save_dir: "/data/ronghao/uenv/uenv-bridge/temp/trajectories/swe",
      trace_format: "jsonl",
      capture_tool_calls: "开启",
      notes: "保留成功、失败和中断轨迹。",
    },
  },
  {
    id: "web-navigation-trajectory",
    category: "trajectory",
    title: "Web 导航轨迹",
    description: "采集 Agent 在网页上的点击、输入、滚动和页面跳转等完整操作链",
    icon: Activity,
    fields: trajectoryFields,
    defaults: {
      run_id: "web_navigation_trace_collect",
      source: "web navigation",
      model_endpoint: "http://127.0.0.1:18194/v1",
      sample_limit: "100",
      episode_max_steps: "30",
      max_response_length: "8192",
      save_dir: "/data/ronghao/uenv/uenv-bridge/temp/trajectories/web_navigation",
      trace_format: "jsonl",
      capture_tool_calls: "开启",
      notes: "保留点击、输入、滚动、跳转和页面状态。",
    },
  },
];

export function getLaunchOption(optionId: string | null | undefined) {
  return launchOptions.find((option) => option.id === optionId) ?? launchOptions[0];
}

function getDefaults(option: LaunchOption, initialRunId: string | null | undefined) {
  return {
    ...option.defaults,
    run_id: initialRunId || option.defaults.run_id,
  };
}

function demoEvent(message: string) {
  return `${new Date().toLocaleTimeString("zh-CN", { hour12: false })}  ${message}`;
}

function actionLabel(category: CategoryId) {
  if (category === "train") return "训练";
  if (category === "benchmark") return "评测";
  return "采集";
}

const homepageParticles = [
  { left: "6%", top: "17%", size: 6, opacity: 0.34, duration: "18s", delay: "0s" },
  { left: "11%", top: "52%", size: 4, opacity: 0.28, duration: "22s", delay: "1s" },
  { left: "18%", top: "73%", size: 5, opacity: 0.3, duration: "24s", delay: "2s" },
  { left: "24%", top: "29%", size: 7, opacity: 0.24, duration: "20s", delay: "0.5s" },
  { left: "34%", top: "61%", size: 4, opacity: 0.32, duration: "19s", delay: "1.5s" },
  { left: "43%", top: "14%", size: 5, opacity: 0.26, duration: "26s", delay: "0s" },
  { left: "52%", top: "79%", size: 7, opacity: 0.22, duration: "21s", delay: "2.5s" },
  { left: "61%", top: "38%", size: 4, opacity: 0.34, duration: "23s", delay: "1s" },
  { left: "71%", top: "18%", size: 6, opacity: 0.28, duration: "25s", delay: "3s" },
  { left: "78%", top: "66%", size: 5, opacity: 0.3, duration: "18s", delay: "1.2s" },
  { left: "86%", top: "33%", size: 7, opacity: 0.24, duration: "22s", delay: "0.8s" },
  { left: "93%", top: "76%", size: 4, opacity: 0.32, duration: "20s", delay: "2.2s" },
  { left: "96%", top: "12%", size: 5, opacity: 0.24, duration: "24s", delay: "1.8s" },
];

export function ParticleField() {
  return (
    <div className="pointer-events-none absolute inset-0 z-0 overflow-hidden" aria-hidden="true">
      <style>
        {`
          @keyframes uenv-particle-drift {
            0% { transform: translate3d(0, 0, 0); }
            100% { transform: translate3d(22px, -28px, 0); }
          }
        `}
      </style>
      {homepageParticles.map((particle) => (
        <span
          key={`${particle.left}-${particle.top}`}
          className="absolute rounded-full bg-[#0070F3]"
          style={{
            left: particle.left,
            top: particle.top,
            width: particle.size,
            height: particle.size,
            opacity: particle.opacity,
            boxShadow: "0 0 18px rgba(0, 112, 243, 0.35)",
            animation: `uenv-particle-drift ${particle.duration} ease-in-out ${particle.delay} infinite alternate`,
          }}
        />
      ))}
    </div>
  );
}

type HomeNavPage = "home" | "progress";

interface ProgressSummaryItem {
  label: string;
  value: string;
}

interface ProgressOverviewCounts {
  total: number;
  running: number;
  completed: number;
  pending: number;
  terminated: number;
  scope: "all" | "loaded";
}

type ProgressFilter = "all" | CategoryId;
type ProgressTaskStatus = "running" | "completed" | "pending" | "terminated";

interface ProgressStep {
  name: string;
  duration: string;
  status: string;
  timestamp?: string;
  dateLabel?: string;
}

interface ProgressTask {
  id: string;
  runId: string;
  category: CategoryId;
  title: string;
  description: string;
  status: ProgressTaskStatus;
  progress: number;
  currentStep: string;
  startTime: string;
  duration: string;
  steps: ProgressStep[];
}

const progressCategoryMeta: Record<CategoryId, { label: string; icon: LucideIcon }> = {
  train: { label: "大模型后训练", icon: BrainCircuit },
  benchmark: { label: "评测", icon: ClipboardCheck },
  trajectory: { label: "轨迹采集", icon: GitBranch },
};

const initialProgressTasks: ProgressTask[] = [
  {
    id: "train-swesmith-grpo",
    runId: "verl_swesmith_grpo_train_demo",
    category: "train",
    title: "SWE-smith GRPO 训练",
    description: "使用 SWE-smith 数据进行代码修复任务后训练，跟踪 rollout、actor update 和模型保存。",
    status: "running",
    progress: 68,
    currentStep: "actor update",
    startTime: "2026-08-22 09:20",
    duration: "7 小时 42 分钟",
    steps: [
      { name: "加载数据", duration: "2 分 18 秒", status: "完成", timestamp: "09:20", dateLabel: "8 月 22 日" },
      { name: "启动 rollout", duration: "41 分 06 秒", status: "完成", timestamp: "09:22", dateLabel: "8 月 22 日" },
      { name: "actor update", duration: "18 分 32 秒", status: "运行中", timestamp: "10:03", dateLabel: "8 月 22 日" },
      { name: "保存 checkpoint", duration: "-", status: "待执行", timestamp: "--", dateLabel: "8 月 22 日" },
    ],
  },
  {
    id: "train-gsm8k-grpo",
    runId: "verl_gsm8k_grpo_train_demo",
    category: "train",
    title: "GSM8K GRPO 训练",
    description: "不接入 UEnv 的 VeRL GRPO 训练，用于验证基础训练脚本和评估流程。",
    status: "completed",
    progress: 100,
    currentStep: "完成",
    startTime: "2026-08-21 18:10",
    duration: "1 小时 16 分钟",
    steps: [
      { name: "准备数据", duration: "1 分 04 秒", status: "完成", timestamp: "18:10", dateLabel: "8 月 21 日" },
      { name: "rollout", duration: "42 分 20 秒", status: "完成", timestamp: "18:11", dateLabel: "8 月 21 日" },
      { name: "actor update", duration: "29 分 11 秒", status: "完成", timestamp: "18:53", dateLabel: "8 月 21 日" },
      { name: "eval", duration: "3 分 33 秒", status: "完成", timestamp: "19:22", dateLabel: "8 月 21 日" },
    ],
  },
  {
    id: "eval-five-benchmark",
    runId: "five_benchmark_eval_demo",
    category: "benchmark",
    title: "五类 Benchmark 评测",
    description: "覆盖 PubMedQA、SciTab、DSCodeBench、SWE 和 Olympic Math 的基准模型评测。",
    status: "completed",
    progress: 100,
    currentStep: "结果归档",
    startTime: "2026-08-20 14:30",
    duration: "3 小时 58 分钟",
    steps: [
      { name: "PubMedQA", duration: "24 分 05 秒", status: "完成", timestamp: "14:30", dateLabel: "8 月 20 日" },
      { name: "SciTab", duration: "19 分 44 秒", status: "完成", timestamp: "14:54", dateLabel: "8 月 20 日" },
      { name: "DSCodeBench", duration: "56 分 10 秒", status: "完成", timestamp: "15:14", dateLabel: "8 月 20 日" },
      { name: "SWE", duration: "2 小时 12 分钟", status: "完成", timestamp: "16:10", dateLabel: "8 月 20 日" },
    ],
  },
  {
    id: "eval-swe-pro",
    runId: "swe_pro_eval_demo",
    category: "benchmark",
    title: "SWE-bench Pro 抽样评测",
    description: "用于检查正式 SWE 训练配置前的环境执行、判分和结果回传路径。",
    status: "pending",
    progress: 20,
    currentStep: "等待执行",
    startTime: "未开始",
    duration: "-",
    steps: [
      { name: "准备样本", duration: "2 分 40 秒", status: "完成", timestamp: "09:00", dateLabel: "8 月 22 日" },
      { name: "启动评测", duration: "-", status: "待执行", timestamp: "--", dateLabel: "8 月 22 日" },
      { name: "结果回传", duration: "-", status: "待执行", timestamp: "--", dateLabel: "8 月 22 日" },
    ],
  },
  {
    id: "trace-swe",
    runId: "swe_trace_collect_demo",
    category: "trajectory",
    title: "SWE 轨迹采集",
    description: "采集代码修复任务中的文件编辑、测试执行和最终 patch。",
    status: "running",
    progress: 46,
    currentStep: "采集中",
    startTime: "2026-08-22 13:05",
    duration: "2 小时 03 分钟",
    steps: [
      { name: "加载实例", duration: "3 分 12 秒", status: "完成", timestamp: "13:05", dateLabel: "8 月 22 日" },
      { name: "执行 agent", duration: "1 小时 48 分钟", status: "运行中", timestamp: "13:08", dateLabel: "8 月 22 日" },
      { name: "写入轨迹", duration: "11 分 20 秒", status: "运行中", timestamp: "14:56", dateLabel: "8 月 22 日" },
    ],
  },
  {
    id: "trace-web-navigation",
    runId: "web_navigation_trace_collect_demo",
    category: "trajectory",
    title: "Web 导航轨迹",
    description: "采集 Agent 在网页上的点击、输入、滚动和页面跳转等完整操作链。",
    status: "pending",
    progress: 10,
    currentStep: "等待配置",
    startTime: "未开始",
    duration: "-",
    steps: [
      { name: "选择网页任务", duration: "-", status: "待执行", timestamp: "--", dateLabel: "8 月 22 日" },
      { name: "启动浏览器环境", duration: "-", status: "待执行", timestamp: "--", dateLabel: "8 月 22 日" },
      { name: "保存操作链", duration: "-", status: "待执行", timestamp: "--", dateLabel: "8 月 22 日" },
    ],
  },
];

const progressPollIntervalMs = 15000;
const progressPageSize = 5;
const progressBackendFetchMaxPages = 100;
const terminatedProgressTaskIdsKey = "uenv-demo-terminated-progress-task-ids";
const terminatedProgressTaskTimesKey = "uenv-demo-terminated-progress-task-times";
const hiddenProgressTaskIdsKey = "uenv-hidden-progress-task-ids";

const workflowStageLabels: Record<WorkflowStage, string> = {
  SUBMIT: "提交任务",
  DISPATCH: "调度下发",
  EXECUTE: "环境执行",
  REPORT: "结果回传",
  DONE: "完成收口",
  FAILED: "失败收口",
};

const workflowStageOrder: WorkflowStage[] = ["SUBMIT", "DISPATCH", "EXECUTE", "REPORT", "DONE", "FAILED"];

function readStringSet(storageKey: string) {
  if (typeof window === "undefined") return new Set<string>();

  try {
    const rawValue = window.localStorage.getItem(storageKey);
    const ids = rawValue ? JSON.parse(rawValue) : [];
    return new Set(Array.isArray(ids) ? ids.filter((id): id is string => typeof id === "string") : []);
  } catch {
    return new Set<string>();
  }
}

function writeStringSet(storageKey: string, values: Set<string>) {
  if (typeof window === "undefined") return;

  try {
    window.localStorage.setItem(storageKey, JSON.stringify([...values]));
  } catch {
    // Local UI preferences can fail silently if browser storage is unavailable.
  }
}

function readNumberRecord(storageKey: string) {
  if (typeof window === "undefined") return {} as Record<string, number>;

  try {
    const rawValue = window.localStorage.getItem(storageKey);
    const parsed = rawValue ? JSON.parse(rawValue) : {};
    if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) return {};
    return Object.fromEntries(
      Object.entries(parsed).filter((entry): entry is [string, number] => {
        const [key, value] = entry;
        return typeof key === "string" && typeof value === "number" && Number.isFinite(value) && value > 0;
      }),
    );
  } catch {
    return {};
  }
}

function writeNumberRecord(storageKey: string, values: Record<string, number>) {
  if (typeof window === "undefined") return;

  try {
    window.localStorage.setItem(storageKey, JSON.stringify(values));
  } catch {
    // Local UI preferences can fail silently if browser storage is unavailable.
  }
}

function readTerminatedProgressTaskIds() {
  return readStringSet(terminatedProgressTaskIdsKey);
}

function readTerminatedProgressTaskTimes() {
  return readNumberRecord(terminatedProgressTaskTimesKey);
}

function readHiddenProgressTaskIds() {
  return readStringSet(hiddenProgressTaskIdsKey);
}

function hasTerminalStep(steps: ProgressStep[]) {
  return steps.some((step) => step.name === "任务完成时间" || step.name === "任务终止时间");
}

function appendLocalTerminationStep(task: ProgressTask, terminatedAt?: number): ProgressStep[] {
  if (!terminatedAt || hasTerminalStep(task.steps)) return task.steps;
  return [
    ...task.steps,
    {
      name: "任务终止时间",
      duration: "-",
      status: "已终止",
      timestamp: formatShortTime(terminatedAt),
      dateLabel: formatMonthDay(terminatedAt),
    },
  ];
}

function applyTerminatedProgressTask(
  task: ProgressTask,
  terminatedIds: Set<string>,
  terminatedTimes: Record<string, number>,
): ProgressTask {
  if (!terminatedIds.has(task.id)) return task;
  return {
    ...task,
    status: "terminated",
    currentStep: "已终止",
    steps: appendLocalTerminationStep(task, terminatedTimes[task.id]),
  };
}

export function isProgressTaskTerminated(taskId: string | null | undefined) {
  if (!taskId) return false;
  return readTerminatedProgressTaskIds().has(taskId);
}

export function markProgressTaskTerminated(taskId: string) {
  const terminatedIds = readTerminatedProgressTaskIds();
  const terminatedTimes = readTerminatedProgressTaskTimes();
  terminatedIds.add(taskId);
  terminatedTimes[taskId] = terminatedTimes[taskId] || Date.now();
  writeStringSet(terminatedProgressTaskIdsKey, terminatedIds);
  writeNumberRecord(terminatedProgressTaskTimesKey, terminatedTimes);
}

function markProgressTasksHidden(taskIds: string[]) {
  const hiddenIds = readHiddenProgressTaskIds();
  for (const taskId of taskIds) hiddenIds.add(taskId);
  writeStringSet(hiddenProgressTaskIdsKey, hiddenIds);
}

function formatDateTime(ts?: number) {
  if (!ts) return "未知";
  return new Date(ts).toLocaleString("zh-CN", { hour12: false });
}

function formatShortTime(ts?: number) {
  if (!ts) return "--";
  return new Date(ts).toLocaleTimeString("zh-CN", {
    hour: "2-digit",
    minute: "2-digit",
    hour12: false,
  });
}

function formatMonthDay(ts?: number) {
  if (!ts) return "--";
  const date = new Date(ts);
  return `${date.getMonth() + 1} 月 ${date.getDate()} 日`;
}

function formatDurationMs(ms: number | undefined) {
  if (!ms || ms <= 0) return "-";
  const totalMinutes = Math.max(1, Math.round(ms / 60_000));
  const days = Math.floor(totalMinutes / 1440);
  const hours = Math.floor((totalMinutes % 1440) / 60);
  const minutes = totalMinutes % 60;
  const parts: string[] = [];
  if (days) parts.push(`${days} 天`);
  if (hours) parts.push(`${hours} 小时`);
  if (minutes || parts.length === 0) parts.push(`${minutes} 分钟`);
  return parts.join(" ");
}

function runIdTimestamp(runId: string) {
  const match = runId.match(/(20\d{6})_(\d{6})/);
  if (!match) return 0;
  const [, date, time] = match;
  const year = Number(date.slice(0, 4));
  const month = Number(date.slice(4, 6)) - 1;
  const day = Number(date.slice(6, 8));
  const hour = Number(time.slice(0, 2));
  const minute = Number(time.slice(2, 4));
  const second = Number(time.slice(4, 6));
  return new Date(year, month, day, hour, minute, second).getTime();
}

function inferTaskCategory(runId: string, state?: ChainState): CategoryId {
  const normalized = runId.toLowerCase();
  if (normalized.includes("trace") || normalized.includes("trajectory") || normalized.includes("collect")) {
    return "trajectory";
  }
  if (
    normalized.includes("eval") ||
    normalized.includes("benchmark") ||
    normalized.includes("pubmed") ||
    normalized.includes("scitab") ||
    normalized.includes("dscode") ||
    normalized.includes("olympic")
  ) {
    return "benchmark";
  }
  const envTypes = Object.values(state?.episodes ?? {})
    .map((episode) => episode.env_type?.toLowerCase() ?? "")
    .join(" ");
  if (envTypes.includes("trajectory")) return "trajectory";
  return "train";
}

function inferTaskTitle(runId: string, category: CategoryId) {
  const normalized = runId.toLowerCase();
  if (normalized.includes("swesmith") || normalized.includes("swe_smith")) return "SWE-smith GRPO 训练";
  if (normalized.includes("swe")) return category === "benchmark" ? "SWE 评测" : "SWE 任务";
  if (normalized.includes("gsm8k")) return "GSM8K GRPO 训练";
  if (normalized.includes("pubmed")) return "PubMedQA 评测";
  if (normalized.includes("scitab")) return "SciTab 评测";
  if (normalized.includes("dscode")) return "DSCodeBench 评测";
  if (normalized.includes("olympic")) return "Olympic Math 评测";
  return progressCategoryMeta[category].label;
}

function isTerminalNodeStatus(status: NodeStatus) {
  return status === "DONE" || status === "FAILED" || status === "SKIPPED" || status === "CLOSED";
}

function statusFromExplicitRunStatus(runStatus?: string): ProgressTaskStatus | null {
  const normalized = (runStatus ?? "").toLowerCase();
  if (normalized === "running" || normalized === "stopping") return "running";
  if (normalized === "completed") return "completed";
  if (normalized === "terminated" || normalized === "failed" || normalized === "cancelled") return "terminated";
  if (normalized === "pending") return "pending";
  return null;
}

function statusFromRunState(state: ChainState): ProgressTaskStatus {
  const explicitStatus = statusFromExplicitRunStatus(state.run_status);
  if (explicitStatus) return explicitStatus;
  if (state.run_state === "RUNNING" || state.run_state === "STOPPING") return "running";
  if (state.run_state === "CLOSED") return "completed";
  return "pending";
}

function rawStatusFromRunSummary(summary: RunSummary): ProgressTaskStatus {
  const explicitStatus = statusFromExplicitRunStatus(summary.run_status);
  if (explicitStatus) return explicitStatus;
  if (summary.run_state === "RUNNING" || summary.run_state === "STOPPING") return "running";
  if (summary.run_state === "CLOSED") return "completed";
  return "pending";
}

function statusFromRunSummary(summary: RunSummary): ProgressTaskStatus {
  return rawStatusFromRunSummary(summary);
}

function stageStatusLabel(statuses: NodeStatus[]) {
  if (statuses.includes("ACTIVE")) return "运行中";
  if (statuses.includes("FAILED")) return "失败";
  if (statuses.includes("PENDING")) return "待执行";
  return "完成";
}

function labelFromStage(stage: string, fallback: string) {
  const normalized = stage.toUpperCase();
  if (normalized in workflowStageLabels) return workflowStageLabels[normalized as WorkflowStage];
  return fallback || "运行中";
}

function timelineStatusLabel(status: string) {
  const normalized = status.toUpperCase();
  if (normalized === "ACTIVE" || normalized === "RUNNING") return "运行中";
  if (normalized === "FAILED") return "失败";
  if (normalized === "PENDING") return "待执行";
  return "完成";
}

function capRunningProgress(status: ProgressTaskStatus, progress: number) {
  return status === "running" ? Math.min(progress, 99) : progress;
}

function positiveNumber(value: unknown) {
  return typeof value === "number" && Number.isFinite(value) && value > 0 ? value : 0;
}

function formatEpisodeProgress(done: number, total: number) {
  return total > 0 ? `${done}/${total} episodes` : `${done} episodes`;
}

function episodeProgressPercent(status: ProgressTaskStatus, done: number, plannedTotal: number) {
  if (status === "completed") return 100;
  if (plannedTotal <= 0) return 0;
  return capRunningProgress(status, Math.max(1, Math.round((done / plannedTotal) * 100)));
}

function timelineActivityText(episodeCount: number, eventCount: number) {
  if (episodeCount > 0) return `${episodeCount} episodes`;
  if (eventCount > 0) return `${eventCount} events`;
  return "-";
}

function timelineDisplayName(item: RunTimelineItem) {
  const label = item.label || labelFromStage(item.stage, "");
  const stage = item.stage.toUpperCase();
  if (["SUBMIT", "DISPATCH", "EXECUTE", "REPORT", "DONE", "FAILED"].includes(stage)) {
    return `最近${label}`;
  }
  return label;
}

function taskStartTimestamp(runId: string, timeline: RunTimelineItem[], fallbackTs: number) {
  const runStartedTs = timeline.find((item) => item.stage.toUpperCase() === "RUN_STARTED")?.first_source_ts ?? 0;
  if (runStartedTs > 0) return runStartedTs;
  const timelineTimestamps = timeline.flatMap((item) => [item.first_source_ts, item.last_source_ts]).filter((ts) => ts > 0);
  if (timelineTimestamps.length > 0) return Math.min(...timelineTimestamps);
  const runIdTs = runIdTimestamp(runId);
  if (runIdTs > 0) return runIdTs;
  return fallbackTs;
}

function taskEndTimestamp(timeline: RunTimelineItem[], fallbackTs: number) {
  const terminalStages = new Set(["RUN_COMPLETED", "RUN_TERMINATED", "RUN_FAILED", "RUN_CLOSED", "RUN_STOPPED", "DONE", "FAILED"]);
  const terminalTimestamps = timeline
    .filter((item) => terminalStages.has(item.stage.toUpperCase()))
    .map((item) => item.last_source_ts || item.first_source_ts)
    .filter((ts) => ts > 0);
  if (terminalTimestamps.length > 0) return Math.max(...terminalTimestamps);
  return fallbackTs;
}

function buildActivityStepsFromTimeline(timeline: RunTimelineItem[]): ProgressStep[] {
  const boundaryStages = new Set([
    "RUN_STARTED",
    "RUN_COMPLETED",
    "RUN_TERMINATED",
    "RUN_FAILED",
    "RUN_CLOSED",
    "RUN_STOPPED",
    "SUBMIT",
    "DONE",
    "FAILED",
  ]);
  return timeline
    .filter((item) => !boundaryStages.has(item.stage.toUpperCase()))
    .map((item) => {
      const ts = item.last_source_ts > 0 ? item.last_source_ts : item.first_source_ts;
      return { item, ts };
    })
    .filter(({ ts }) => ts > 0)
    .sort((a, b) => a.ts - b.ts)
    .map(({ item, ts }) => ({
      name: timelineDisplayName(item),
      duration: timelineActivityText(item.episode_count, item.event_count),
      status: timelineStatusLabel(item.status),
      timestamp: formatShortTime(ts),
      dateLabel: formatMonthDay(ts),
    }));
}

function buildTaskTimelineSteps(
  status: ProgressTaskStatus,
  startTs: number,
  endTs: number,
  activitySteps: ProgressStep[],
): ProgressStep[] {
  const steps: ProgressStep[] = [
    {
      name: "任务提交时间",
      duration: "-",
      status: "已提交",
      timestamp: formatShortTime(startTs),
      dateLabel: formatMonthDay(startTs),
    },
  ];

  steps.push(...activitySteps);

  if (status === "completed" || status === "terminated") {
    steps.push({
      name: status === "completed" ? "任务完成时间" : "任务终止时间",
      duration: formatDurationMs(endTs - startTs),
      status: status === "completed" ? "已完成" : "已终止",
      timestamp: formatShortTime(endTs),
      dateLabel: formatMonthDay(endTs),
    });
  }

  return steps.filter((step, index) => {
    if (index === 0) return true;
    return step.timestamp !== "--" || step.name === "任务完成时间" || step.name === "任务终止时间";
  });
}

function buildProgressSteps(state: ChainState): ProgressStep[] {
  const nodes = state.workflow?.nodes ?? [];
  const nodesByStage = new Map<WorkflowStage, typeof nodes>();

  for (const node of nodes) {
    const stage = node.stage ?? "SUBMIT";
    const stageNodes = nodesByStage.get(stage) ?? [];
    stageNodes.push(node);
    nodesByStage.set(stage, stageNodes);
  }

  const steps = workflowStageOrder.flatMap((stage) => {
    const stageNodes = nodesByStage.get(stage) ?? [];
    if (stageNodes.length === 0) return [];
    const timestamps = stageNodes.map((node) => node.source_ts).filter((ts) => ts > 0);
    const firstTs = timestamps.length ? Math.min(...timestamps) : undefined;
    const statuses = stageNodes.map((node) => node.status);
    const status = stageStatusLabel(statuses);
    const episodeCount =
      typeof stageNodes[0]?.payload_summary?.count === "number" ? stageNodes[0].payload_summary.count : 0;

    return [
      {
        name: workflowStageLabels[stage],
        duration: timelineActivityText(episodeCount, stageNodes.length),
        status,
        timestamp: formatShortTime(firstTs),
        dateLabel: formatMonthDay(firstTs),
      },
    ];
  });

  if (steps.length > 0) return steps;

  return [
    {
      name: state.run_state === "RUNNING" ? "运行中" : "等待事件",
      duration: "-",
      status: state.run_state === "RUNNING" ? "运行中" : "待执行",
      timestamp: formatShortTime(state.updated_at),
      dateLabel: formatMonthDay(state.updated_at),
    },
  ];
}

function progressTaskFromChainState(state: ChainState, timeline: RunTimelineItem[] = []): ProgressTask {
  const runId = state.training_run_id;
  const category = inferTaskCategory(runId, state);
  const title = inferTaskTitle(runId, category);
  const episodes = Object.values(state.episodes ?? {});
  const activitySteps = buildActivityStepsFromTimeline(timeline);
  const terminalEpisodes = episodes.filter((episode) => isTerminalNodeStatus(episode.status));
  const activeNode = [...(state.workflow?.nodes ?? [])]
    .filter((node) => node.status === "ACTIVE")
    .sort((a, b) => (b.source_ts ?? 0) - (a.source_ts ?? 0))[0];
  const activeTimelineItem = timeline.find((item) => item.status.toUpperCase() === "ACTIVE");
  const status = statusFromRunState(state);
  const plannedEpisodeTotal = positiveNumber(state.planned_episode_total);
  const episodeProgressTotal = plannedEpisodeTotal;
  const allObservedEpisodesFinished = episodes.length > 0 && terminalEpisodes.length === episodes.length;
  const waitingForNextEpisodeBatch =
    status === "running" && allObservedEpisodesFinished && (!plannedEpisodeTotal || terminalEpisodes.length < plannedEpisodeTotal);
  const progress =
    plannedEpisodeTotal > 0
      ? episodeProgressPercent(status, terminalEpisodes.length, plannedEpisodeTotal)
      : status === "completed"
        ? 100
        : 0;
  const timestamps = [
    ...(state.workflow?.nodes ?? []).map((node) => node.source_ts),
    ...episodes.map((episode) => episode.last_source_ts),
  ].filter((ts) => ts > 0);
  const startTs = taskStartTimestamp(runId, timeline, timestamps.length ? Math.min(...timestamps) : state.updated_at);
  const endTs =
    status === "completed" || status === "terminated" ? taskEndTimestamp(timeline, state.updated_at) : Date.now();
  const currentStep = activeTimelineItem?.label
    ? activeTimelineItem.label
    : activeNode?.stage
    ? workflowStageLabels[activeNode.stage]
    : status === "completed"
      ? "完成"
      : status === "terminated"
        ? "已终止"
      : status === "running"
        ? waitingForNextEpisodeBatch
          ? "等待下一批 episode"
          : "运行中"
        : "等待执行";

  return {
    id: runId,
    runId,
    category,
    title,
    description: `${progressCategoryMeta[category].label} · ${formatEpisodeProgress(terminalEpisodes.length, episodeProgressTotal)} · ${state.run_state}`,
    status,
    progress,
    currentStep,
    startTime: formatDateTime(startTs),
    duration: formatDurationMs(endTs - startTs),
    steps: buildTaskTimelineSteps(
      status,
      startTs,
      endTs,
      activitySteps.length > 0
        ? activitySteps
        : buildProgressSteps(state).filter((step) => !["提交任务", "完成收口", "失败收口"].includes(step.name)),
    ),
  };
}

function progressTaskFromRunSummary(summary: RunSummary): ProgressTask {
  const runId = summary.training_run_id;
  const category = inferTaskCategory(runId);
  const title = inferTaskTitle(runId, category);
  const status = statusFromRunSummary(summary);
  const finishedEpisodes = summary.episode_done + summary.episode_failed;
  const plannedEpisodeTotal = positiveNumber(summary.planned_episode_total);
  const episodeProgressTotal = plannedEpisodeTotal;
  const allObservedEpisodesFinished = summary.episode_total > 0 && finishedEpisodes === summary.episode_total;
  const waitingForNextEpisodeBatch =
    status === "running" && allObservedEpisodesFinished && (!plannedEpisodeTotal || finishedEpisodes < plannedEpisodeTotal);
  const progress =
    plannedEpisodeTotal > 0
      ? episodeProgressPercent(status, finishedEpisodes, plannedEpisodeTotal)
      : status === "completed"
        ? 100
        : 0;
  const currentStep =
    status === "completed"
      ? "完成"
      : status === "terminated"
        ? "已终止"
      : status === "running" && waitingForNextEpisodeBatch
        ? "等待下一批 episode"
      : labelFromStage(summary.active_stage, summary.active_stage_label);
  const durationEnd = status === "completed" || status === "terminated" ? summary.updated_at : Date.now();
  const stepStatus =
    status === "completed" ? "完成" : status === "terminated" ? "已终止" : status === "running" ? "运行中" : "待执行";
  const activitySteps =
    status === "running" || status === "pending"
      ? [
          {
            name: currentStep,
            duration: timelineActivityText(summary.episode_total, summary.global_event_seq),
            status: stepStatus,
            timestamp: formatShortTime(summary.updated_at),
            dateLabel: formatMonthDay(summary.updated_at),
          },
        ]
      : [];

  return {
    id: runId,
    runId,
    category,
    title,
    description: `${progressCategoryMeta[category].label} · ${formatEpisodeProgress(finishedEpisodes, episodeProgressTotal)} · ${summary.run_state}`,
    status,
    progress,
    currentStep,
    startTime: formatDateTime(summary.started_at),
    duration: formatDurationMs(durationEnd - summary.started_at),
    steps: buildTaskTimelineSteps(status, summary.started_at, durationEnd, activitySteps),
  };
}

function progressTaskFromRunId(runId: string): ProgressTask {
  const category = inferTaskCategory(runId);
  const timestamp = runIdTimestamp(runId);
  const startTime = timestamp > 0 ? timestamp : Date.now();
  return {
    id: runId,
    runId,
    category,
    title: inferTaskTitle(runId, category),
    description: `${progressCategoryMeta[category].label} · 等待摘要数据`,
    status: "pending",
    progress: 0,
    currentStep: "等待摘要数据",
    startTime: formatDateTime(startTime),
    duration: "-",
    steps: [
      {
        name: "等待摘要数据",
        duration: "-",
        status: "待执行",
        timestamp: formatShortTime(startTime),
        dateLabel: formatMonthDay(startTime),
      },
    ],
  };
}

function applyLocalProgressTaskState(tasks: ProgressTask[]) {
  const hiddenIds = readHiddenProgressTaskIds();
  const terminatedIds = readTerminatedProgressTaskIds();
  const terminatedTimes = readTerminatedProgressTaskTimes();
  return tasks
    .filter((task) => !hiddenIds.has(task.id))
    .map((task) => applyTerminatedProgressTask(task, terminatedIds, terminatedTimes));
}

function countProgressTasks(tasks: ProgressTask[], scope: ProgressOverviewCounts["scope"]): ProgressOverviewCounts {
  return {
    total: tasks.length,
    running: tasks.filter((task) => task.status === "running").length,
    completed: tasks.filter((task) => task.status === "completed").length,
    pending: tasks.filter((task) => task.status === "pending").length,
    terminated: tasks.filter((task) => task.status === "terminated").length,
    scope,
  };
}

function applyStatusCountDelta(
  counts: ProgressOverviewCounts,
  fromStatus: ProgressTaskStatus,
  toStatus: ProgressTaskStatus,
) {
  if (fromStatus === toStatus) return counts;
  const next = { ...counts };
  if (fromStatus === "running") next.running = Math.max(0, next.running - 1);
  if (fromStatus === "completed") next.completed = Math.max(0, next.completed - 1);
  if (fromStatus === "pending") next.pending = Math.max(0, next.pending - 1);
  if (fromStatus === "terminated") next.terminated = Math.max(0, next.terminated - 1);
  if (toStatus === "running") next.running += 1;
  if (toStatus === "completed") next.completed += 1;
  if (toStatus === "pending") next.pending += 1;
  if (toStatus === "terminated") next.terminated += 1;
  return next;
}

function overviewFromStatusCounts(
  counts: RunSummaryStatusCounts,
  summaries: RunSummary[],
): ProgressOverviewCounts {
  let overview: ProgressOverviewCounts = {
    total: counts.total,
    running: counts.running,
    completed: counts.completed,
    pending: counts.pending,
    terminated: counts.terminated ?? 0,
    scope: "all",
  };

  for (const summary of summaries) {
    overview = applyStatusCountDelta(overview, rawStatusFromRunSummary(summary), statusFromRunSummary(summary));
  }
  return overview;
}

function subtractProgressOverviewTasks(
  counts: ProgressOverviewCounts,
  tasks: ProgressTask[],
): ProgressOverviewCounts {
  const next = { ...counts };
  for (const task of tasks) {
    next.total = Math.max(0, next.total - 1);
    if (task.status === "running") next.running = Math.max(0, next.running - 1);
    if (task.status === "completed") next.completed = Math.max(0, next.completed - 1);
    if (task.status === "pending") next.pending = Math.max(0, next.pending - 1);
    if (task.status === "terminated") next.terminated = Math.max(0, next.terminated - 1);
  }
  return next;
}

function getFixtureProgressTasks(pageIndex: number) {
  const offset = pageIndex * progressPageSize;
  return applyLocalProgressTaskState(initialProgressTasks).slice(offset, offset + progressPageSize);
}

interface ProgressTaskPageResult {
  tasks: ProgressTask[];
  overview: ProgressOverviewCounts;
  total?: number;
  hasNextPage?: boolean;
}

async function fetchBackendSummaryProgressTasks(
  client: AggregationClient,
  pageIndex: number,
): Promise<ProgressTaskPageResult> {
  const pageStart = pageIndex * progressPageSize;
  const pageEnd = pageStart + progressPageSize;
  const targetVisibleCount = pageEnd + 1;
  const visibleTasks: ProgressTask[] = [];
  const fetchedSummaries: RunSummary[] = [];
  let total: number | undefined;
  let statusCounts: RunSummaryStatusCounts | undefined;
  let offset = 0;
  let reachedEnd = false;

  for (let pageCount = 0; pageCount < progressBackendFetchMaxPages; pageCount += 1) {
    const page = await client.listRunSummaryPage({ limit: progressPageSize, offset });
    fetchedSummaries.push(...page.runs);
    visibleTasks.push(...applyLocalProgressTaskState(page.runs.map(progressTaskFromRunSummary)));
    if (typeof page.total === "number") total = page.total;
    if (page.status_counts) statusCounts = page.status_counts;

    offset += progressPageSize;
    if (page.runs.length < progressPageSize || (typeof total === "number" && offset >= total)) {
      reachedEnd = true;
      break;
    }
    if (visibleTasks.length >= targetVisibleCount) break;
  }

  return {
    tasks: visibleTasks.slice(pageStart, pageEnd),
    overview: statusCounts
      ? overviewFromStatusCounts(statusCounts, fetchedSummaries)
      : countProgressTasks(visibleTasks, "loaded"),
    total,
    hasNextPage: visibleTasks.length > pageEnd || (!reachedEnd && visibleTasks.length >= pageEnd),
  };
}

async function fetchBackendProgressTasks(client: AggregationClient, pageIndex: number): Promise<ProgressTaskPageResult> {
  try {
    return await fetchBackendSummaryProgressTasks(client, pageIndex);
  } catch {
    // Older Obs deployments may not expose /runs/summary yet. Keep the list
    // lightweight by showing run_id-level placeholders instead of pulling full
    // multi-MB state for every run.
  }

  const pageStart = pageIndex * progressPageSize;
  const pageEnd = pageStart + progressPageSize;
  const targetVisibleCount = pageEnd + 1;
  const visibleTasks: ProgressTask[] = [];
  let offset = 0;
  let reachedEnd = false;

  for (let pageCount = 0; pageCount < progressBackendFetchMaxPages; pageCount += 1) {
    const runIds = await client.listRuns({ limit: progressPageSize, offset });
    visibleTasks.push(...applyLocalProgressTaskState(runIds.map(progressTaskFromRunId)));
    offset += progressPageSize;
    if (runIds.length < progressPageSize) {
      reachedEnd = true;
      break;
    }
    if (visibleTasks.length >= targetVisibleCount) break;
  }

  return {
    tasks: visibleTasks.slice(pageStart, pageEnd),
    overview: countProgressTasks(visibleTasks, "loaded"),
    hasNextPage: visibleTasks.length > pageEnd || (!reachedEnd && visibleTasks.length >= pageEnd),
  };
}

function useProgressTasks(pageIndex: number, refreshKey = 0) {
  const [aggregationConfig] = useState(() => getAggregationConfig());
  const [tasks, setTasks] = useState<ProgressTask[]>(() =>
    aggregationConfig.useFixture ? getFixtureProgressTasks(pageIndex) : [],
  );
  const [overviewCounts, setOverviewCounts] = useState<ProgressOverviewCounts>(() =>
    aggregationConfig.useFixture
      ? countProgressTasks(applyLocalProgressTaskState(initialProgressTasks), "all")
      : countProgressTasks([], "loaded"),
  );
  const [loading, setLoading] = useState(!aggregationConfig.useFixture);
  const [error, setError] = useState<string | null>(null);
  const [hasNextPage, setHasNextPage] = useState(() => {
    if (!aggregationConfig.useFixture) return true;
    return applyLocalProgressTaskState(initialProgressTasks).length > progressPageSize;
  });

  useEffect(() => {
    if (aggregationConfig.useFixture || !aggregationConfig.baseUrl) {
      const fixtureTasks = getFixtureProgressTasks(pageIndex);
      const allFixtureTasks = applyLocalProgressTaskState(initialProgressTasks);
      setTasks(fixtureTasks);
      setOverviewCounts(countProgressTasks(allFixtureTasks, "all"));
      setHasNextPage(allFixtureTasks.length > (pageIndex + 1) * progressPageSize);
      setLoading(false);
      setError(null);
      return undefined;
    }

    const client = new AggregationClient(aggregationConfig.baseUrl, aggregationConfig.token);
    let cancelled = false;

    async function load() {
      setLoading(true);
      try {
        const result = await fetchBackendProgressTasks(client, pageIndex);
        if (cancelled) return;
        setTasks(result.tasks);
        setOverviewCounts(result.overview);
        setHasNextPage(
          typeof result.hasNextPage === "boolean"
            ? result.hasNextPage
            : typeof result.total === "number"
            ? (pageIndex + 1) * progressPageSize < result.total
            : result.tasks.length === progressPageSize,
        );
        setError(null);
      } catch (err) {
        if (cancelled) return;
        const message = err instanceof Error ? err.message : "获取后端任务失败";
        setError(message);
      } finally {
        if (!cancelled) setLoading(false);
      }
    }

    void load();
    const timer = window.setInterval(() => void load(), progressPollIntervalMs);

    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, [aggregationConfig.baseUrl, aggregationConfig.token, aggregationConfig.useFixture, pageIndex, refreshKey]);

  return {
    tasks,
    setTasks,
    overviewCounts,
    setOverviewCounts,
    loading,
    error,
    usingBackend: !aggregationConfig.useFixture,
    hasNextPage,
    pageSize: progressPageSize,
  };
}

export function useProgressTask(taskId: string | null | undefined) {
  const [aggregationConfig] = useState(() => getAggregationConfig());
  const [task, setTask] = useState<ProgressTask | null>(() => {
    const fallback = getProgressTask(taskId);
    return fallback ? applyLocalProgressTaskState([fallback])[0] ?? null : null;
  });
  const [loading, setLoading] = useState(!aggregationConfig.useFixture);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!taskId) {
      setTask(null);
      setLoading(false);
      setError(null);
      return undefined;
    }

    if (aggregationConfig.useFixture || !aggregationConfig.baseUrl) {
      const fallback = getProgressTask(taskId);
      setTask(fallback ? applyLocalProgressTaskState([fallback])[0] ?? null : null);
      setLoading(false);
      setError(null);
      return undefined;
    }

    const client = new AggregationClient(aggregationConfig.baseUrl, aggregationConfig.token);
    let cancelled = false;

    async function load() {
      setLoading(true);
      try {
        const [state, timeline] = await Promise.all([
          client.getState(taskId!),
          client.getRunTimeline(taskId!).catch(() => []),
        ]);
        if (cancelled) return;
        setTask(applyLocalProgressTaskState([progressTaskFromChainState(state, timeline)])[0] ?? null);
        setError(null);
      } catch (err) {
        if (cancelled) return;
        const fallback = getProgressTask(taskId);
        setTask(fallback ? applyLocalProgressTaskState([fallback])[0] ?? null : null);
        const message = err instanceof Error ? err.message : "获取后端任务失败";
        setError(message);
      } finally {
        if (!cancelled) setLoading(false);
      }
    }

    void load();
    const timer = window.setInterval(() => void load(), progressPollIntervalMs);

    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, [aggregationConfig.baseUrl, aggregationConfig.token, aggregationConfig.useFixture, taskId]);

  return {
    task,
    loading,
    error,
    usingBackend: !aggregationConfig.useFixture,
  };
}

function progressStatusLabel(status: ProgressTaskStatus) {
  if (status === "running") return "运行中";
  if (status === "completed") return "已完成";
  if (status === "terminated") return "已终止";
  return "待处理";
}

function progressStatusClass(status: ProgressTaskStatus) {
  if (status === "running") return "border-[#0070F3]/20 bg-[#0070F3]/5 text-[#0070F3]";
  if (status === "completed") return "border-emerald-200 bg-emerald-50 text-emerald-700";
  if (status === "terminated") return "border-rose-200 bg-rose-50 text-rose-700";
  return "border-gray-200 bg-gray-50 text-gray-500";
}

export function getProgressTask(taskId: string | null | undefined) {
  const task = initialProgressTasks.find((item) => item.id === taskId) ?? null;
  return task ? applyLocalProgressTaskState([task])[0] ?? null : null;
}

function homeNavLinkClass(active: boolean) {
  return `inline-flex h-8 items-center rounded-full px-3 text-sm font-medium transition ${
    active ? "bg-[#111111] text-white" : "text-gray-500 hover:bg-gray-100 hover:text-[#111111]"
  }`;
}

function HomeNavigation({ active }: { active: HomeNavPage }) {
  const aggregationConfig = getAggregationConfig();
  const leftBadge = aggregationConfig.useFixture ? "本地 Demo" : "Server Obs";
  const rightBadge = aggregationConfig.useFixture ? "未接后端" : "";

  return (
    <nav className="sticky top-0 z-20 border-b border-gray-200/20 bg-white/75 backdrop-blur-md">
      <div className="mx-auto flex h-16 max-w-5xl items-center justify-between gap-4 px-6">
        <a href="/" className="shrink-0 text-lg font-bold tracking-tight text-[#111111]">
          UEnv
        </a>
        <div className="flex items-center gap-1 rounded-full border border-gray-200 bg-white/70 p-1">
          <a href="/" className={homeNavLinkClass(active === "home")} aria-current={active === "home" ? "page" : undefined}>
            首页
          </a>
          <a
            href="/progress"
            className={homeNavLinkClass(active === "progress")}
            aria-current={active === "progress" ? "page" : undefined}
          >
            进展
          </a>
        </div>
        <div className="hidden items-center gap-2 text-sm sm:flex">
          <span className="inline-flex h-8 items-center rounded-full border border-[#0070F3]/20 px-3 font-medium text-[#0070F3]">
            {leftBadge}
          </span>
          {rightBadge && (
            <span className="inline-flex h-8 items-center rounded-full border border-gray-200 px-3 font-medium text-gray-500">
              {rightBadge}
            </span>
          )}
        </div>
      </div>
    </nav>
  );
}

function Field({
  field,
  value,
  onChange,
}: {
  field: FieldConfig;
  value: string;
  onChange: (value: string) => void;
}) {
  const wrapperClass = field.wide ? "grid gap-1.5 text-sm md:col-span-2" : "grid gap-1.5 text-sm";

  return (
    <label className={wrapperClass}>
      <span className="font-medium text-slate-700">{field.label}</span>
      {field.type === "select" ? (
        <select
          value={value}
          onChange={(event) => onChange(event.target.value)}
          className="h-10 rounded-md border border-slate-200 bg-white px-3 text-sm text-slate-900 outline-none transition focus:border-cyan-400 focus:ring-2 focus:ring-cyan-100"
        >
          {(field.options ?? []).map((option) => (
            <option key={option} value={option}>
              {option}
            </option>
          ))}
        </select>
      ) : field.type === "textarea" ? (
        <textarea
          value={value}
          rows={4}
          placeholder={field.placeholder}
          onChange={(event) => onChange(event.target.value)}
          className="min-h-24 resize-y rounded-md border border-slate-200 bg-white px-3 py-2 text-sm text-slate-900 outline-none transition focus:border-cyan-400 focus:ring-2 focus:ring-cyan-100"
        />
      ) : (
        <input
          value={value}
          type={field.type ?? "text"}
          placeholder={field.placeholder}
          onChange={(event) => onChange(event.target.value)}
          className="h-10 rounded-md border border-slate-200 bg-white px-3 text-sm text-slate-900 outline-none transition focus:border-cyan-400 focus:ring-2 focus:ring-cyan-100"
        />
      )}
    </label>
  );
}

function HomeCategory({
  category,
  options,
  expanded,
  onToggle,
}: {
  category: CategoryConfig;
  options: LaunchOption[];
  expanded: boolean;
  onToggle: () => void;
}) {
  const Icon = category.icon;

  return (
    <section className="group overflow-hidden rounded-2xl border border-gray-200 bg-gray-50/60 p-1 transition duration-200 ease-in-out hover:-translate-y-1 hover:bg-white hover:shadow-[0_24px_80px_rgba(0,0,0,0.08)]">
      <button
        type="button"
        aria-expanded={expanded}
        onClick={onToggle}
        className="flex w-full items-start justify-between gap-8 rounded-xl px-8 py-8 text-left transition duration-200 ease-in-out"
      >
        <span className="flex min-w-0 items-center gap-6">
          <span className={`flex h-16 w-16 shrink-0 items-center justify-center rounded-2xl border ${category.tone}`}>
            <Icon className="h-8 w-8" />
          </span>
          <span className="min-w-0">
            <span className="block text-3xl font-bold tracking-tight text-[#111111]">{category.title}</span>
            <span className="mt-3 block max-w-2xl text-base font-normal leading-relaxed text-gray-500">
              {category.subtitle}
            </span>
          </span>
        </span>
        <ChevronDown
          className={`mt-2 h-5 w-5 shrink-0 text-gray-400 transition duration-200 ease-in-out ${expanded ? "rotate-180" : ""}`}
        />
      </button>
      {expanded && (
        <div className="grid gap-3 border-t border-gray-200/70 px-5 pb-5 pt-4 md:grid-cols-2 lg:grid-cols-3">
          {options.map((option) => {
            const OptionIcon = option.icon;
            return (
              <a
                key={option.id}
                href={`/launch?option=${encodeURIComponent(option.id)}`}
                className="grid gap-2 rounded-2xl border border-gray-200 bg-white px-4 py-4 text-left text-gray-700 transition duration-200 ease-in-out hover:-translate-y-1 hover:border-[#0070F3]/40 hover:shadow-[0_16px_48px_rgba(0,112,243,0.10)]"
              >
                <span className="flex items-center justify-between gap-3">
                  <span className="flex min-w-0 items-center gap-2">
                    <OptionIcon className="h-4 w-4 shrink-0 text-[#0070F3]" />
                    <span className="text-sm font-bold tracking-tight text-[#111111]">{option.title}</span>
                  </span>
                  <ChevronRight className="h-4 w-4 shrink-0 text-gray-300" />
                </span>
                <span className="text-xs font-normal leading-relaxed text-gray-500">{option.description}</span>
              </a>
            );
          })}
          <div className="grid gap-2 rounded-2xl border border-dashed border-gray-200 bg-gray-50 px-4 py-4 text-left text-gray-400">
            <span className="flex min-w-0 items-center gap-2">
              <Icon className="h-4 w-4 shrink-0" />
              <span className="text-sm font-bold tracking-tight">{category.comingSoonTitle}</span>
            </span>
            <span className="text-xs font-normal leading-relaxed">{category.comingSoonDescription}</span>
          </div>
        </div>
      )}
    </section>
  );
}

function LaunchPanel({
  option,
  values,
  demoState,
  demoProgress,
  demoEvents,
  backendRun,
  backendEnabled,
  onValueChange,
  onStart,
  onStop,
  onReset,
}: {
  option: LaunchOption;
  values: Record<string, string>;
  demoState: DemoRunState;
  demoProgress: number;
  demoEvents: string[];
  backendRun: ActiveBackendRun | null;
  backendEnabled: boolean;
  onValueChange: (key: string, value: string) => void;
  onStart: () => void;
  onStop: () => void;
  onReset: () => void;
}) {
  const OptionIcon = option.icon;
  const verb = actionLabel(option.category);
  const statusMeta: Record<DemoRunState, { label: string; className: string; dot: string }> = {
    idle: {
      label: "待启动",
      className: "border-slate-200 bg-slate-50 text-slate-600",
      dot: "bg-slate-400",
    },
    running: {
      label: "运行中",
      className: "border-cyan-200 bg-cyan-50 text-cyan-700",
      dot: "bg-cyan-500",
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
    failed: {
      label: "启动失败",
      className: "border-rose-200 bg-rose-50 text-rose-700",
      dot: "bg-rose-500",
    },
  };
  const status = statusMeta[demoState];

  return (
    <section className="rounded-lg border border-slate-200 bg-white">
      <div className="border-b border-slate-100 px-5 py-4">
        <div className="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
          <div className="flex min-w-0 items-center gap-3">
            <div className="flex h-11 w-11 shrink-0 items-center justify-center rounded-md border border-cyan-200 bg-cyan-50 text-cyan-700">
              <OptionIcon className="h-5 w-5" />
            </div>
            <div className="min-w-0">
              <h2 className="text-xl font-semibold tracking-normal text-slate-950">{option.title}</h2>
              <p className="mt-1 text-sm leading-5 text-slate-500">{option.description}</p>
            </div>
          </div>
          <span className={`inline-flex h-8 items-center gap-2 rounded-md border px-3 text-xs font-medium ${status.className}`}>
            <span className={`h-2 w-2 rounded-full ${status.dot}`} />
            {status.label}
          </span>
        </div>
      </div>

      <div className="grid gap-5 p-5 xl:grid-cols-[minmax(0,1fr)_320px]">
        <div className="grid gap-4 md:grid-cols-2">
          {option.fields.map((field) => (
            <Field
              key={field.key}
              field={field}
              value={values[field.key] ?? ""}
              onChange={(value) => onValueChange(field.key, value)}
            />
          ))}
        </div>

        <aside className="grid content-start gap-4">
          <div className="rounded-lg border border-slate-200 bg-slate-50 p-4">
            <div className="flex items-center justify-between gap-3">
              <p className="text-sm font-semibold text-slate-900">任务状态</p>
              <span className="text-xs tabular-nums text-slate-500">{Math.round(demoProgress)}%</span>
            </div>
            <div className="mt-3 h-2 overflow-hidden rounded-full bg-white">
              <div
                className="h-full rounded-full bg-cyan-600 transition-all"
                style={{ width: `${Math.min(100, Math.max(0, demoProgress))}%` }}
              />
            </div>
            <div className="mt-4 grid grid-cols-2 gap-2">
              <button
                type="button"
                disabled={demoState === "running" || demoState === "stopping"}
                onClick={onStart}
                className="inline-flex h-10 items-center justify-center gap-2 rounded-md bg-cyan-700 px-3 text-sm font-semibold text-white transition hover:bg-cyan-800 disabled:cursor-not-allowed disabled:bg-slate-300"
              >
                <Play className="h-4 w-4" />
                启动{verb}
              </button>
              <button
                type="button"
                disabled={demoState !== "running"}
                onClick={onStop}
                className="inline-flex h-10 items-center justify-center gap-2 rounded-md border border-rose-200 bg-white px-3 text-sm font-semibold text-rose-700 transition hover:bg-rose-50 disabled:cursor-not-allowed disabled:border-slate-200 disabled:text-slate-300"
              >
                <Square className="h-4 w-4" />
                终止
              </button>
            </div>
            <button
              type="button"
              onClick={onReset}
              className="mt-2 inline-flex h-9 w-full items-center justify-center gap-2 rounded-md border border-slate-200 bg-white px-3 text-sm font-medium text-slate-600 transition hover:border-slate-300 hover:bg-slate-100"
            >
              <RotateCcw className="h-4 w-4" />
              重置
            </button>
          </div>

          {backendRun ? (
            <div className="rounded-lg border border-cyan-100 bg-cyan-50/70 p-4 text-sm text-slate-700">
              <div className="mb-3 font-semibold text-slate-900">后端任务</div>
              <div className="grid gap-2">
                <div className="flex items-center justify-between gap-3">
                  <span className="text-slate-500">run_id</span>
                  <span className="truncate font-mono text-xs text-slate-900">{backendRun.run_id}</span>
                </div>
                <div className="flex items-center justify-between gap-3">
                  <span className="text-slate-500">pid</span>
                  <span className="font-mono text-xs text-slate-900">{backendRun.pid}</span>
                </div>
                <a
                  href={backendRun.progress_path}
                  className="mt-2 inline-flex h-9 items-center justify-center rounded-md bg-cyan-700 px-3 text-sm font-semibold text-white transition hover:bg-cyan-800"
                >
                  打开进度
                </a>
                <a
                  href={backendRun.server_path}
                  className="inline-flex h-9 items-center justify-center rounded-md border border-cyan-200 bg-white px-3 text-sm font-semibold text-cyan-700 transition hover:bg-cyan-50"
                >
                  打开 server
                </a>
                <div className="mt-2 grid gap-1 border-t border-cyan-100 pt-3 font-mono text-[11px] leading-5 text-slate-500">
                  <span className="truncate">log: {backendRun.log_file}</span>
                  <span className="truncate">service: {backendRun.service_dir}</span>
                </div>
              </div>
            </div>
          ) : null}

          <div className="rounded-lg border border-slate-200 bg-white p-4">
            <div className="mb-3 flex items-center gap-2 text-sm font-semibold text-slate-900">
              <Gauge className="h-4 w-4 text-cyan-700" />
              {backendEnabled ? "任务事件" : "本地事件"}
            </div>
            <div className="max-h-52 overflow-auto rounded-md border border-slate-200 bg-slate-950 p-3 font-mono text-[11px] leading-5 text-slate-100">
              {demoEvents.map((event) => (
                <div key={event}>{event}</div>
              ))}
            </div>
          </div>
        </aside>
      </div>
    </section>
  );
}

export function UserLaunchConsole() {
  const [expanded, setExpanded] = useState<Record<CategoryId, boolean>>({
    train: true,
    benchmark: true,
    trajectory: true,
  });

  return (
    <main
      className="relative min-h-screen overflow-hidden bg-[#FFFFFF] text-[#111111]"
      style={{ fontFamily: "'Inter', -apple-system, BlinkMacSystemFont, sans-serif" }}
    >
      <ParticleField />
      <HomeNavigation active="home" />

      <div className="relative z-10 mx-auto max-w-5xl px-6">
        <section className="py-24 text-center md:py-32">
          <div className="mx-auto max-w-5xl">
            <h1 className="mt-6 text-7xl font-bold leading-none tracking-tight text-[#111111] md:text-8xl lg:text-[9rem]">
              UEnv
            </h1>
            <h2 className="mx-auto mt-8 max-w-4xl text-4xl font-bold tracking-tight text-[#111111] md:text-5xl">
              统一的大模型任务入口
            </h2>
          </div>
        </section>

        <section className="flex items-center gap-3 pb-6">
          <div className="h-6 w-1 rounded-full bg-[#0070F3]" />
          <h3 className="flex items-center gap-2 text-2xl font-bold tracking-tight text-[#111111]">
            <Layers3 className="h-5 w-5 text-[#0070F3]" />
            选择任务类型
          </h3>
        </section>

        <section className="grid gap-6 pb-24 md:pb-32">
          {categories.map((category) => (
            <HomeCategory
              key={category.id}
              category={category}
              options={launchOptions.filter((option) => option.category === category.id)}
              expanded={expanded[category.id]}
              onToggle={() => setExpanded((current) => ({ ...current, [category.id]: !current[category.id] }))}
            />
          ))}
        </section>
      </div>
    </main>
  );
}

export function UserProgressPage() {
  const [pageIndex, setPageIndex] = useState(0);
  const [progressRefreshKey, setProgressRefreshKey] = useState(0);
  const {
    tasks,
    setTasks,
    overviewCounts,
    setOverviewCounts,
    loading,
    error,
    usingBackend,
    hasNextPage,
    pageSize,
  } = useProgressTasks(pageIndex, progressRefreshKey);
  const [filter, setFilter] = useState<ProgressFilter>("all");
  const [selectedTaskIds, setSelectedTaskIds] = useState<string[]>([]);
  const [deleteDialogOpen, setDeleteDialogOpen] = useState(false);

  const visibleTasks = filter === "all" ? tasks : tasks.filter((task) => task.category === filter);
  const selectedTaskSet = new Set(selectedTaskIds);
  const selectedTasks = tasks.filter((task) => selectedTaskSet.has(task.id));
  const deleteIncludesRunning = selectedTasks.some((task) => task.status === "running");
  const allVisibleSelected = visibleTasks.length > 0 && visibleTasks.every((task) => selectedTaskSet.has(task.id));
  const progressSummary: ProgressSummaryItem[] = [
    { label: overviewCounts.scope === "all" ? "任务总数" : "已加载任务", value: String(overviewCounts.total) },
    { label: "运行中", value: String(overviewCounts.running) },
    { label: "已完成", value: String(overviewCounts.completed) },
    { label: "待处理", value: String(overviewCounts.pending) },
    { label: "已终止", value: String(overviewCounts.terminated) },
  ];
  const categoryFilters: Array<{ id: ProgressFilter; label: string; count: number }> = [
    { id: "all", label: "全部", count: tasks.length },
    { id: "train", label: progressCategoryMeta.train.label, count: tasks.filter((task) => task.category === "train").length },
    {
      id: "benchmark",
      label: progressCategoryMeta.benchmark.label,
      count: tasks.filter((task) => task.category === "benchmark").length,
    },
    {
      id: "trajectory",
      label: progressCategoryMeta.trajectory.label,
      count: tasks.filter((task) => task.category === "trajectory").length,
    },
  ];

  function changeFilter(nextFilter: ProgressFilter) {
    setFilter(nextFilter);
    setSelectedTaskIds([]);
  }

  useEffect(() => {
    setSelectedTaskIds((current) => current.filter((taskId) => tasks.some((task) => task.id === taskId)));
  }, [tasks]);

  function toggleTaskSelection(taskId: string) {
    setSelectedTaskIds((current) =>
      current.includes(taskId) ? current.filter((id) => id !== taskId) : [...current, taskId],
    );
  }

  function toggleVisibleSelection() {
    if (allVisibleSelected) {
      setSelectedTaskIds([]);
      return;
    }

    setSelectedTaskIds(visibleTasks.map((task) => task.id));
  }

  function requestDeleteSelectedTasks() {
    if (selectedTaskIds.length === 0) return;
    setDeleteDialogOpen(true);
  }

  function goToPreviousPage() {
    setPageIndex((current) => Math.max(0, current - 1));
    setSelectedTaskIds([]);
  }

  function goToNextPage() {
    if (!hasNextPage) return;
    setPageIndex((current) => current + 1);
    setSelectedTaskIds([]);
  }

  function confirmDeleteSelectedTasks() {
    const selectedIds = new Set(selectedTaskIds);
    markProgressTasksHidden(selectedTaskIds);
    setTasks((current) => current.filter((task) => !selectedIds.has(task.id)));
    setOverviewCounts((current) => subtractProgressOverviewTasks(current, selectedTasks));
    setSelectedTaskIds([]);
    setDeleteDialogOpen(false);
    setProgressRefreshKey((current) => current + 1);
  }

  return (
    <main
      className="relative min-h-screen overflow-hidden bg-[#FFFFFF] text-[#111111]"
      style={{ fontFamily: "'Inter', -apple-system, BlinkMacSystemFont, sans-serif" }}
    >
      <ParticleField />
      <HomeNavigation active="progress" />

      <div className="relative z-10 mx-auto max-w-5xl px-6 py-16 md:py-20">
        <section className="pb-10">
          <h1 className="mt-4 text-5xl font-bold tracking-tight text-[#111111] md:text-6xl">
            任务进展
          </h1>
          <div className="mt-4 flex flex-wrap items-center gap-2 text-sm text-gray-500">
            <span className="inline-flex h-8 items-center rounded-full border border-[#0070F3]/20 bg-[#0070F3]/5 px-3 font-medium text-[#0070F3]">
              {usingBackend ? "实时数据" : "本地 Demo 数据"}
            </span>
            <span>
              {usingBackend
                ? loading
                  ? "正在同步当前页"
                  : ``
                : ``}
            </span>
          </div>
          {error ? (
            <p className="mt-3 rounded-2xl border border-amber-200 bg-amber-50 px-4 py-3 text-sm text-amber-800">
              {error}
            </p>
          ) : null}
        </section>

        <section className="pb-10">
          <div className="mb-4 flex items-center gap-3">
            <div className="h-6 w-1 rounded-full bg-[#0070F3]" />
            <h2 className="flex items-center gap-2 text-2xl font-bold tracking-tight text-[#111111]">
              <Gauge className="h-5 w-5 text-[#0070F3]" />
              进度总览
            </h2>
          </div>
          <div className="grid gap-3 sm:grid-cols-2 lg:grid-cols-5">
            {progressSummary.map((item) => (
              <div key={item.label} className="rounded-2xl border border-gray-200 bg-white p-5 shadow-[0_16px_48px_rgba(0,0,0,0.04)]">
                <p className="text-sm font-medium text-gray-500">{item.label}</p>
                <p className="mt-3 text-4xl font-bold tracking-tight text-[#111111]">{item.value}</p>
              </div>
            ))}
          </div>
        </section>

        <section className="pb-24">
          <div className="mb-4 flex items-center gap-3">
            <div className="h-6 w-1 rounded-full bg-[#0070F3]" />
            <h2 className="flex items-center gap-2 text-2xl font-bold tracking-tight text-[#111111]">
              <Layers3 className="h-5 w-5 text-[#0070F3]" />
              任务细节
            </h2>
          </div>

          <div className="rounded-2xl border border-gray-200 bg-gray-50/70 p-1">
            <div className="rounded-xl bg-white p-5">
              <div className="mb-5 flex flex-col gap-3 lg:flex-row lg:items-center lg:justify-between">
                <div className="flex flex-wrap gap-2">
                  {categoryFilters.map((item) => (
                    <button
                      key={item.id}
                      type="button"
                      onClick={() => changeFilter(item.id)}
                      className={`inline-flex h-9 items-center gap-2 rounded-full border px-3 text-sm font-medium transition ${
                        filter === item.id
                          ? "border-[#0070F3]/20 bg-[#0070F3] text-white"
                          : "border-gray-200 bg-white text-gray-500 hover:border-[#0070F3]/30 hover:text-[#111111]"
                      }`}
                    >
                      {item.label}
                      <span className={filter === item.id ? "text-white/70" : "text-gray-400"}>{item.count}</span>
                    </button>
                  ))}
                </div>
                <div className="flex flex-wrap gap-2">
                  <button
                    type="button"
                    onClick={toggleVisibleSelection}
                    disabled={visibleTasks.length === 0}
                    className="inline-flex h-9 items-center rounded-full border border-gray-200 bg-white px-3 text-sm font-medium text-gray-600 transition hover:border-[#0070F3]/30 hover:text-[#111111] disabled:cursor-not-allowed disabled:text-gray-300"
                  >
                    {allVisibleSelected ? "取消选择" : "选择当前分类"}
                  </button>
                  <button
                    type="button"
                    onClick={requestDeleteSelectedTasks}
                    disabled={selectedTaskIds.length === 0}
                    className="inline-flex h-9 items-center rounded-full border border-rose-200 bg-white px-3 text-sm font-medium text-rose-600 transition hover:bg-rose-50 disabled:cursor-not-allowed disabled:border-gray-200 disabled:text-gray-300"
                  >
                    删除选中 {selectedTaskIds.length > 0 ? selectedTaskIds.length : ""}
                  </button>
                </div>
              </div>

              <div className="grid gap-3">
                  {visibleTasks.length === 0 ? (
                    <div className="rounded-2xl border border-dashed border-gray-200 bg-gray-50 px-5 py-10 text-center text-sm text-gray-400">
                      {loading ? "正在同步任务" : "当前分类暂无任务"}
                    </div>
                  ) : (
                    visibleTasks.map((task) => {
                      const categoryMeta = progressCategoryMeta[task.category];
                      const Icon = categoryMeta.icon;
                      return (
                        <article
                          key={task.id}
                          className="rounded-2xl border border-gray-200 bg-white p-4 transition hover:border-[#0070F3]/30 hover:bg-[#0070F3]/[0.03]"
                        >
                          <div className="flex items-start gap-3">
                            <input
                              type="checkbox"
                              checked={selectedTaskSet.has(task.id)}
                              onClick={(event) => event.stopPropagation()}
                              onChange={(event) => {
                                event.stopPropagation();
                                toggleTaskSelection(task.id);
                              }}
                              className="mt-1 h-4 w-4 rounded border-gray-300 text-[#0070F3]"
                              aria-label={`选择 ${task.title}`}
                            />
                            <a href={`/progress/${encodeURIComponent(task.id)}`} className="min-w-0 flex-1">
                              <div className="flex items-start justify-between gap-3">
                                <div className="flex min-w-0 items-center gap-2">
                                  <Icon className="h-4 w-4 shrink-0 text-[#0070F3]" />
                                  <h3 className="truncate font-mono text-sm font-bold tracking-tight text-[#111111]">
                                    {task.runId}
                                  </h3>
                                </div>
                                <span className="flex shrink-0 items-center gap-2">
                                  <span className={`rounded-full border px-2 py-0.5 text-[11px] font-medium ${progressStatusClass(task.status)}`}>
                                    {progressStatusLabel(task.status)}
                                  </span>
                                  <ChevronRight className="h-4 w-4 text-gray-300" />
                                </span>
                              </div>
                              <div className="mt-3 text-xs text-gray-500">
                                <span className="inline-flex items-center gap-1.5">
                                  <span className="h-1.5 w-1.5 rounded-full bg-[#0070F3]" />
                                  {categoryMeta.label}
                                </span>
                              </div>
                              <div className="mt-3 flex items-center gap-3">
                                <div className="h-2 flex-1 overflow-hidden rounded-full bg-gray-100">
                                  <div className="h-full rounded-full bg-[#0070F3]" style={{ width: `${task.progress}%` }} />
                                </div>
                                <span className="w-10 text-right text-xs font-medium text-gray-500">{task.progress}%</span>
                              </div>
                            </a>
                          </div>
                        </article>
                      );
                    })
                  )}
                </div>

              <div className="mt-5 flex flex-col gap-3 border-t border-gray-100 pt-4 sm:flex-row sm:items-center sm:justify-between">
                <p className="text-sm text-gray-500">
                  第 {pageIndex + 1} 页 · 当前加载 {tasks.length} 条
                </p>
                <div className="flex items-center gap-2">
                  <button
                    type="button"
                    onClick={goToPreviousPage}
                    disabled={pageIndex === 0 || loading}
                    className="inline-flex h-9 items-center rounded-full border border-gray-200 bg-white px-3 text-sm font-medium text-gray-600 transition hover:border-[#0070F3]/30 hover:text-[#111111] disabled:cursor-not-allowed disabled:text-gray-300"
                  >
                    上一页
                  </button>
                  <button
                    type="button"
                    onClick={goToNextPage}
                    disabled={!hasNextPage || loading}
                    className="inline-flex h-9 items-center rounded-full border border-gray-200 bg-white px-3 text-sm font-medium text-gray-600 transition hover:border-[#0070F3]/30 hover:text-[#111111] disabled:cursor-not-allowed disabled:text-gray-300"
                  >
                    下一页
                  </button>
                </div>
              </div>
            </div>
          </div>
        </section>
      </div>

      {deleteDialogOpen ? (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/30 px-4 backdrop-blur-sm">
          <section
            role="dialog"
            aria-modal="true"
            aria-labelledby="delete-task-dialog-title"
            className="w-full max-w-md rounded-2xl border border-gray-200 bg-white p-6 shadow-[0_24px_80px_rgba(0,0,0,0.18)]"
          >
            <h2 id="delete-task-dialog-title" className="text-xl font-bold tracking-tight text-[#111111]">
              确认删除任务
            </h2>
            <div className="mt-4 space-y-3 text-sm leading-relaxed text-gray-600">
              <p>删除后不可恢复。</p>
              {deleteIncludesRunning ? <p>选中的任务中包含正在运行的任务，删除会中断该任务。是否确认删除？</p> : null}
            </div>
            <div className="mt-6 flex justify-end gap-2">
              <button
                type="button"
                onClick={() => setDeleteDialogOpen(false)}
                className="inline-flex h-10 items-center rounded-full border border-gray-200 bg-white px-4 text-sm font-medium text-gray-600 transition hover:bg-gray-50 hover:text-[#111111]"
              >
                取消
              </button>
              <button
                type="button"
                onClick={confirmDeleteSelectedTasks}
                className="inline-flex h-10 items-center rounded-full border border-rose-600 bg-rose-600 px-4 text-sm font-medium text-white transition hover:bg-rose-700"
              >
                确认删除
              </button>
            </div>
          </section>
        </div>
      ) : null}
    </main>
  );
}

export function UserLaunchConfigPage({
  optionId,
  initialRunId = null,
}: {
  optionId?: string | null;
  initialRunId?: string | null;
}) {
  const selectedOption = getLaunchOption(optionId);
  const [formValues, setFormValues] = useState<Record<string, string>>(() =>
    getDefaults(selectedOption, initialRunId),
  );
  const [demoState, setDemoState] = useState<DemoRunState>("idle");
  const [demoProgress, setDemoProgress] = useState(0);
  const [demoEvents, setDemoEvents] = useState<string[]>(["等待启动"]);
  const [backendRun, setBackendRun] = useState<ActiveBackendRun | null>(null);
  const [backendMode, setBackendMode] = useState(false);
  const backendEnabled = selectedOption.id === "verl";

  useEffect(() => {
    setFormValues(getDefaults(selectedOption, initialRunId));
    setDemoState("idle");
    setDemoProgress(0);
    setDemoEvents([demoEvent(`已选择 ${selectedOption.title}`)]);
    setBackendRun(null);
    setBackendMode(false);
  }, [selectedOption.id, selectedOption.title, initialRunId]);

  useEffect(() => {
    if (demoState !== "running") return undefined;
    if (backendMode) return undefined;

    const timer = window.setInterval(() => {
      setDemoProgress((current) => {
        const next = Math.min(100, current + 6);
        if (next >= 100) {
          setDemoState("completed");
          setDemoEvents((events) => [demoEvent("本地 Demo 完成"), ...events].slice(0, 10));
        }
        return next;
      });
    }, 1100);

    return () => window.clearInterval(timer);
  }, [backendMode, demoState]);

  useEffect(() => {
    if (demoState !== "stopping") return undefined;
    if (backendMode) return undefined;

    const timer = window.setTimeout(() => {
      setDemoState("stopped");
      setDemoEvents((events) => [demoEvent("本地 Demo 已终止"), ...events].slice(0, 10));
    }, 700);

    return () => window.clearTimeout(timer);
  }, [backendMode, demoState]);

  function updateValue(key: string, value: string) {
    setFormValues((current) => ({ ...current, [key]: value }));
  }

  function errorMessage(error: unknown) {
    return error instanceof Error ? error.message : String(error);
  }

  async function startTask() {
    const verb = actionLabel(selectedOption.category);
    if (backendEnabled) {
      setBackendMode(true);
      setDemoState("running");
      setDemoProgress(2);
      setBackendRun(null);
      setDemoEvents([
        demoEvent(`提交后端任务: ${selectedOption.title}`),
        demoEvent("加载 VeRL SWE-smith preset 参数"),
      ]);

      try {
        const result = await launchVerlTraining({
          data: {
            option_id: "verl",
            run_id: formValues.run_id ?? "",
            model_path: formValues.model_path ?? "",
            dataset_path: formValues.dataset_path ?? "",
            rl_algorithm: "GRPO",
            limit: formValues.limit ?? "0",
            offset: formValues.offset ?? "0",
            training_steps: formValues.training_steps ?? "null",
            total_epochs: formValues.total_epochs ?? "1",
            train_batch_size: formValues.train_batch_size ?? "2",
            ppo_mini_batch_size: formValues.ppo_mini_batch_size ?? "2",
            rollout_n: formValues.rollout_n ?? "4",
            temperature: formValues.temperature ?? "1.0",
            episode_max_steps: formValues.episode_max_steps ?? "50",
            max_prompt_length: formValues.max_prompt_length ?? "8192",
            max_response_length: formValues.max_response_length ?? "8192",
            parallel_mode: formValues.parallel_mode === "fully_async" ? "fully_async" : "sync",
            save_freq: formValues.save_freq ?? "5",
          },
        });

        setBackendRun(result);
        setDemoProgress(5);
        setDemoEvents([
          demoEvent("VeRL 训练进程已在后端启动"),
          demoEvent(`run_id: ${result.run_id}`),
          demoEvent(`pid: ${result.pid}`),
          demoEvent(`log: ${result.log_file}`),
        ]);
      } catch (error) {
        setBackendMode(false);
        setDemoState("failed");
        setDemoProgress(0);
        setDemoEvents([
          demoEvent(`启动失败: ${errorMessage(error)}`),
          demoEvent("请检查前端服务所在机器是否能访问 uenv-bridge 与训练环境"),
        ]);
      }
      return;
    }

    setDemoState("running");
    setDemoProgress(5);
    setDemoEvents([
      demoEvent(`启动${verb}: ${selectedOption.title}`),
      demoEvent(`run_id: ${formValues.run_id || selectedOption.defaults.run_id}`),
      demoEvent("参数已加载"),
    ]);
  }

  async function stopTask() {
    if (backendMode && backendRun) {
      setDemoState("stopping");
      setDemoEvents((events) => [demoEvent("请求终止后端训练进程"), ...events].slice(0, 10));
      try {
        const result = await stopVerlTraining({
          data: {
            run_id: backendRun.run_id,
            pid: backendRun.pid,
          },
        });
        setDemoState("stopped");
        setDemoProgress(100);
        setDemoEvents((events) =>
          [
            demoEvent(result.status === "signaled" ? "已发送终止信号" : "进程已不存在"),
            ...events,
          ].slice(0, 10),
        );
      } catch (error) {
        setDemoState("failed");
        setDemoEvents((events) => [demoEvent(`终止失败: ${errorMessage(error)}`), ...events].slice(0, 10));
      }
      return;
    }

    setDemoState("stopping");
    setDemoEvents((events) => [demoEvent("请求终止"), ...events].slice(0, 10));
  }

  function resetDemo() {
    setDemoState("idle");
    setDemoProgress(0);
    setDemoEvents([demoEvent(`已选择 ${selectedOption.title}`)]);
    setBackendRun(null);
    setBackendMode(false);
  }

  return (
    <main className="min-h-screen bg-[#f5f7fb] text-slate-900">
      <div className="mx-auto flex min-h-screen max-w-[1760px] flex-col px-4 py-5 lg:px-8">
        <nav className="sticky top-0 z-20 -mx-4 border-b border-gray-200/20 bg-transparent px-4 backdrop-blur-md lg:-mx-8 lg:px-8">
          <div className="mx-auto flex h-16 max-w-[1760px] items-center justify-between px-2 lg:px-0">
            <div className="flex items-center gap-4">
              <a href="/" className="flex items-center gap-1.5 text-sm font-medium text-slate-500 transition hover:text-slate-900">
                <ArrowLeft className="h-4 w-4" />
                返回首页
              </a>
            </div>
            <div className="flex items-center gap-2 text-sm">
              <span className="inline-flex h-8 items-center rounded-full border border-cyan-200 bg-cyan-50 px-3 text-xs font-medium text-cyan-700">
                {backendEnabled ? "后端已接入" : "本地 Demo"}
              </span>
              <span className="inline-flex h-8 items-center rounded-full border border-gray-200 px-3 text-xs font-medium text-gray-500">
                {backendEnabled ? "VeRL preset" : "未接后端"}
              </span>
            </div>
          </div>
        </nav>

        <header className="flex flex-col gap-4 border-b border-slate-200 pb-5 pt-5 lg:flex-row lg:items-end lg:justify-between">
          <div>
            <h1 className="mt-1 text-3xl font-semibold tracking-normal text-slate-950 lg:text-4xl">
              参数配置
            </h1>
          </div>
        </header>

        <section className="mt-5 flex-1">
          <LaunchPanel
            option={selectedOption}
            values={formValues}
            demoState={demoState}
            demoProgress={demoProgress}
            demoEvents={demoEvents}
            backendRun={backendRun}
            backendEnabled={backendEnabled}
            onValueChange={updateValue}
            onStart={startTask}
            onStop={stopTask}
            onReset={resetDemo}
          />
        </section>
      </div>
    </main>
  );
}
