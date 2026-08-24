import { createFileRoute } from "@tanstack/react-router";
import { UserLaunchConfigPage } from "@/components/user-launch-console";

export const Route = createFileRoute("/launch")({
  head: () => ({
    meta: [
      { title: "UEnv · 参数配置" },
      {
        name: "description",
        content: "Configure UEnv training, evaluation, and trajectory collection tasks.",
      },
      { property: "og:title", content: "UEnv · 参数配置" },
      {
        property: "og:description",
        content: "Configure a UEnv task before launching a local demo run.",
      },
    ],
  }),
  component: Launch,
});

function Launch() {
  const search = Route.useSearch() as { option?: string; run?: string };

  return <UserLaunchConfigPage optionId={search.option} initialRunId={search.run} />;
}
