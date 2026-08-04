import { createFileRoute } from "@tanstack/react-router";
import { TrainingConsole } from "@/components/training-console";

export const Route = createFileRoute("/")({
  validateSearch: (search: Record<string, unknown>) => ({
    run: typeof search.run === "string" && search.run.trim() ? search.run.trim() : null,
  }),
  head: () => ({
    meta: [
      { title: "UEnv · Training Run Visualization" },
      {
        name: "description",
        content: "Operational console for observing distributed UEnv training runs in real time.",
      },
      { property: "og:title", content: "UEnv · Training Run Visualization" },
      {
        property: "og:description",
        content: "Operational console for observing distributed UEnv training runs in real time.",
      },
    ],
  }),
  component: Index,
});

function Index() {
  const { run } = Route.useSearch();
  return <TrainingConsole initialRunId={run} />;
}
