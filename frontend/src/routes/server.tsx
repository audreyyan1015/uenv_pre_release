import { createFileRoute } from "@tanstack/react-router";
import { EpisodeJourney } from "@/components/episode-journey";

export const Route = createFileRoute("/server")({
  validateSearch: (search: Record<string, unknown>) => ({
    run: typeof search.run === "string" && search.run.trim() ? search.run.trim() : null,
  }),
  head: () => ({
    meta: [
      { title: "UEnv · Episode 进度" },
      { name: "description", content: "面向使用者的 UEnv Episode 处理进度页面。" },
      { property: "og:title", content: "UEnv · Episode 进度" },
      { property: "og:description", content: "查看每条 Episode 的处理进度与结果状态。" },
    ],
  }),
  component: Server,
});

function Server() {
  const { run } = Route.useSearch();
  return <EpisodeJourney initialRunId={run} />;
}
