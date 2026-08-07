import { createFileRoute } from "@tanstack/react-router";
import { TrainingConsole } from "@/components/training-console";

export const Route = createFileRoute("/ops")({
  validateSearch: (search: Record<string, unknown>) => ({
    run: typeof search.run === "string" && search.run.trim() ? search.run.trim() : null,
  }),
  head: () => ({
    meta: [
      { title: "UEnv · 技术观测台" },
      {
        name: "description",
        content: "Operational console for observing distributed UEnv training runs in real time.",
      },
      { property: "og:title", content: "UEnv · 技术观测台" },
      {
        property: "og:description",
        content: "Observe distributed UEnv training runs in real time.",
      },
    ],
  }),
  component: Ops,
});

function Ops() {
  const { run } = Route.useSearch();
  return <TrainingConsole initialRunId={run} />;
}
