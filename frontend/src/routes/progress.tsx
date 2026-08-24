import { createFileRoute } from "@tanstack/react-router";
import { UserProgressPage } from "@/components/user-launch-console";

export const Route = createFileRoute("/progress")({
  head: () => ({
    meta: [
      { title: "UEnv · 任务进展" },
      {
        name: "description",
        content: "Track UEnv training, benchmark evaluation, and trajectory collection progress.",
      },
      { property: "og:title", content: "UEnv · 任务进展" },
      {
        property: "og:description",
        content: "View UEnv task progress across training, evaluation, and trajectory collection.",
      },
    ],
  }),
  component: Progress,
});

function Progress() {
  return <UserProgressPage />;
}
