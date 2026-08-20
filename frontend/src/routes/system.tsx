import { createFileRoute } from "@tanstack/react-router";

import { SystemTopology } from "@/components/system-topology";

export const Route = createFileRoute("/system")({
  validateSearch: (search: Record<string, unknown>) => ({
    run: typeof search.run === "string" && search.run.trim() ? search.run.trim() : undefined,
  }),
  head: () => ({
    meta: [
      { title: "UEnv · 系统拓扑" },
      {
        name: "description",
        content: "UEnv adapter、server、执行节点、环境资源池与 hub 统一动态拓扑。",
      },
      { property: "og:title", content: "UEnv · 系统拓扑" },
      { property: "og:description", content: "查看 UEnv 当前调度状态与测试进度。" },
    ],
  }),
  component: System,
});

function System() {
  const { run } = Route.useSearch();
  return <SystemTopology initialRunId={run} />;
}
