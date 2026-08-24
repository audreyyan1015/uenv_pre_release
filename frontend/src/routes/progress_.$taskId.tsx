import { useEffect, useState } from "react";
import { createFileRoute, Link } from "@tanstack/react-router";
import {
  ArrowLeft,
  Clock3,
  Square,
} from "lucide-react";
import {
  isProgressTaskTerminated,
  markProgressTaskTerminated,
  ParticleField,
  useProgressTask,
} from "@/components/user-launch-console";

export const Route = createFileRoute("/progress_/$taskId")({
  head: ({ params }) => ({
    meta: [
      { title: `UEnv · ${params.taskId}` },
      {
        name: "description",
        content: "Task detail page with timeline, step breakdown, and server link.",
      },
    ],
  }),
  component: ProgressTaskDetail,
});

function categoryLabel(category: "train" | "benchmark" | "trajectory") {
  if (category === "train") return "大模型后训练";
  if (category === "benchmark") return "评测";
  return "轨迹采集";
}

function statusLabel(status: "running" | "completed" | "pending" | "terminated") {
  if (status === "running") return "运行中";
  if (status === "completed") return "已完成";
  if (status === "terminated") return "已终止";
  return "待处理";
}

function ProgressTaskDetail() {
  const { taskId } = Route.useParams();
  const { task: baseTask, loading, error, usingBackend } = useProgressTask(taskId);
  const [stopDialogOpen, setStopDialogOpen] = useState(false);
  const [locallyTerminated, setLocallyTerminated] = useState(false);

  useEffect(() => {
    setStopDialogOpen(false);
    setLocallyTerminated(isProgressTaskTerminated(taskId));
  }, [taskId]);

  if (!baseTask) {
    return (
      <main className="min-h-screen bg-white px-6 py-20 text-slate-900">
        <div className="mx-auto max-w-4xl">
          <Link to="/progress" className="inline-flex items-center gap-2 text-sm font-medium text-slate-500">
            <ArrowLeft className="h-4 w-4" />
            返回进展
          </Link>
          <p className="mt-8 text-2xl font-bold">{loading ? "正在加载任务" : "未找到任务"}</p>
          {error ? <p className="mt-3 text-sm text-amber-700">{error}</p> : null}
        </div>
      </main>
    );
  }

  const task = locallyTerminated ? { ...baseTask, status: "terminated" as const, currentStep: "已终止" } : baseTask;
  const canStopTask = task.status === "running";

  function confirmStopTask() {
    markProgressTaskTerminated(task.id);
    setLocallyTerminated(true);
    setStopDialogOpen(false);
  }

  return (
    <main className="relative min-h-screen overflow-hidden bg-[#FFFFFF] text-[#111111]" style={{ fontFamily: "'Inter', -apple-system, BlinkMacSystemFont, sans-serif" }}>
      <ParticleField />
      <div className="relative z-10 mx-auto max-w-5xl px-6 py-8">
        <div className="flex items-center justify-between gap-4 border-b border-gray-200 pb-5">
          <Link to="/progress" className="inline-flex items-center gap-2 text-sm font-medium text-slate-500 hover:text-slate-900">
            <ArrowLeft className="h-4 w-4" />
            返回进展
          </Link>
          <div className="flex items-center gap-2">
            {canStopTask ? (
              <button
                type="button"
                onClick={() => setStopDialogOpen(true)}
                className="inline-flex h-9 items-center gap-1.5 rounded-full border border-rose-200 bg-rose-50 px-3 text-sm font-medium text-rose-700 transition hover:bg-rose-100"
              >
                <Square className="h-3.5 w-3.5" />
                停止
              </button>
            ) : null}
            <a
              href={`/server?run=${encodeURIComponent(task.runId)}`}
              target="_blank"
              rel="noreferrer"
              className="inline-flex h-9 items-center rounded-full border border-[#0070F3]/20 bg-[#0070F3]/5 px-3 text-sm font-medium text-[#0070F3]"
            >
              查看 server 详情
            </a>
          </div>
        </div>

        <section className="pt-10">
          <h1 className="mt-3 break-all font-mono text-4xl font-bold tracking-tight">{task.runId}</h1>
          <div className="mt-2 flex flex-wrap items-center gap-2 text-sm font-medium text-[#0070F3]">
            <span>{categoryLabel(task.category)}</span>
            <span className="text-gray-300">/</span>
            <span>{usingBackend ? "实时数据" : "本地 Demo 数据"}</span>
          </div>
          {error ? <p className="mt-3 text-sm text-amber-700">{error}</p> : null}
        </section>

        <section className="mt-10">
          <div className="mb-4 flex items-center gap-3">
            <div className="h-6 w-1 rounded-full bg-[#0070F3]" />
            <h2 className="flex items-center gap-2 text-2xl font-bold tracking-tight text-[#111111]">
              <Clock3 className="h-5 w-5 text-[#0070F3]" />
              时间轴
            </h2>
          </div>
          <div className="divide-y divide-gray-200">
            {task.steps.map((step) => (
              <div key={`${task.id}-${step.name}`} className="grid gap-3 py-4 md:grid-cols-[140px_minmax(0,1fr)] md:items-center">
                <div className="min-w-0">
                  <p className="truncate text-sm font-semibold text-[#111111]">{step.timestamp ?? "--"}</p>
                  <p className="text-xs text-gray-400">{step.dateLabel ?? "--"}</p>
                </div>
                <div className="flex items-center justify-between gap-3">
                  <div className="flex items-center gap-3">
                    <span className="h-3 w-3 shrink-0 rounded-full bg-[#0070F3]" />
                    <div>
                    <p className="text-sm font-semibold text-[#111111]">{step.name}</p>
                    <p className="text-xs text-gray-400">{step.status}</p>
                    </div>
                  </div>
                  <p className="text-right text-sm font-medium text-gray-500">{step.duration}</p>
                </div>
              </div>
            ))}
          </div>
        </section>

        <section className="mt-8 grid gap-8">
          <div>
            <table className="w-full border-collapse text-sm">
              <tbody className="divide-y divide-gray-200">
                <tr>
                  <th className="w-28 py-3 pr-4 text-left font-medium text-gray-400">开始时间</th>
                  <td className="py-3 text-right text-[#111111]">{task.startTime}</td>
                </tr>
                <tr>
                  <th className="w-28 py-3 pr-4 text-left font-medium text-gray-400">总耗时</th>
                  <td className="py-3 text-right text-[#111111]">{task.duration}</td>
                </tr>
                <tr>
                  <th className="w-28 py-3 pr-4 text-left font-medium text-gray-400">当前阶段</th>
                  <td className="py-3 text-right text-[#111111]">{task.currentStep}</td>
                </tr>
                <tr>
                  <th className="w-28 py-3 pr-4 text-left font-medium text-gray-400">状态</th>
                  <td className="py-3 text-right text-[#111111]">{statusLabel(task.status)}</td>
                </tr>
              </tbody>
            </table>
          </div>

          <div className="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
            <div className="max-w-3xl">
              <p className="text-sm font-semibold text-[#111111]">更多细节</p>
              <p className="mt-1 text-sm leading-relaxed text-gray-500">
                点击右上角按钮可跳转到 server 页面查看当前任务的 episode 进度、workflow 状态和更多执行细节。
              </p>
            </div>
            <a
              href={`/server?run=${encodeURIComponent(task.runId)}`}
              target="_blank"
              rel="noreferrer"
              className="inline-flex h-9 shrink-0 items-center rounded-full border border-[#0070F3]/20 bg-[#0070F3]/5 px-3 text-sm font-medium text-[#0070F3]"
            >
              打开 server 详情
            </a>
          </div>
        </section>
      </div>

      {stopDialogOpen ? (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/30 px-4 backdrop-blur-sm">
          <section
            role="dialog"
            aria-modal="true"
            aria-labelledby="stop-task-dialog-title"
            className="w-full max-w-md rounded-2xl border border-gray-200 bg-white p-6 shadow-[0_24px_80px_rgba(0,0,0,0.18)]"
          >
            <h2 id="stop-task-dialog-title" className="text-xl font-bold tracking-tight text-[#111111]">
              确认停止任务
            </h2>
            <div className="mt-4 space-y-3 text-sm leading-relaxed text-gray-600">
              <p>该任务正在运行，停止会中断当前任务。是否确认停止？</p>
              <p>停止后任务状态将变为已终止。</p>
            </div>
            <div className="mt-6 flex justify-end gap-2">
              <button
                type="button"
                onClick={() => setStopDialogOpen(false)}
                className="inline-flex h-10 items-center rounded-full border border-gray-200 bg-white px-4 text-sm font-medium text-gray-600 transition hover:bg-gray-50 hover:text-[#111111]"
              >
                取消
              </button>
              <button
                type="button"
                onClick={confirmStopTask}
                className="inline-flex h-10 items-center rounded-full border border-rose-600 bg-rose-600 px-4 text-sm font-medium text-white transition hover:bg-rose-700"
              >
                确认停止
              </button>
            </div>
          </section>
        </div>
      ) : null}
    </main>
  );
}
