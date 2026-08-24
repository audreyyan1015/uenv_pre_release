import { createFileRoute } from "@tanstack/react-router";
import { UserLaunchConsole } from "@/components/user-launch-console";

export const Route = createFileRoute("/")({
  head: () => ({
    meta: [
      { title: "UEnv · 训练与评测控制台" },
      {
        name: "description",
        content: "User-facing console for launching UEnv training and benchmark evaluation tasks.",
      },
      { property: "og:title", content: "UEnv · 训练与评测控制台" },
      {
        property: "og:description",
        content: "Launch UEnv training and benchmark evaluation tasks from one page.",
      },
    ],
  }),
  component: Index,
});

function Index() {
  return <UserLaunchConsole />;
}
