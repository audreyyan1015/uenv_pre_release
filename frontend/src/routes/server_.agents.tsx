import { createFileRoute } from "@tanstack/react-router";

import { AgentPoolStatus } from "@/components/agent-pool-status";

export const Route = createFileRoute("/server_/agents")({
  head: () => ({
    meta: [
      { title: "UEnv · Agent 池状态" },
      {
        name: "description",
        content: "查看 OpenHands Agent 池容量、实例、任务与执行节点对齐状态。",
      },
      { property: "og:title", content: "UEnv · Agent 池状态" },
      {
        property: "og:description",
        content: "查看 OpenHands Agent 池容量、实例、任务与执行节点对齐状态。",
      },
    ],
  }),
  component: AgentPoolStatus,
});
