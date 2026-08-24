import { Link } from "@tanstack/react-router";
import { Bot, Network } from "lucide-react";

import { buttonVariants } from "@/components/ui/button";
import { cn } from "@/lib/utils";

export function SystemHomeLink({
  className,
  label = "系统拓扑",
}: {
  className?: string;
  label?: string;
}) {
  return (
    <Link
      to="/system"
      search={{ run: undefined }}
      className={cn(buttonVariants({ variant: "outline", size: "sm" }), className)}
      aria-label="返回系统拓扑"
    >
      <Network className="h-4 w-4" />
      {label}
    </Link>
  );
}

export function AgentPoolLink({
  className,
  label = "Agent 池",
}: {
  className?: string;
  label?: string;
}) {
  return (
    <Link
      to="/server/agents"
      className={cn(buttonVariants({ variant: "outline", size: "sm" }), className)}
      aria-label="查看 Agent 池状态"
    >
      <Bot className="h-4 w-4" />
      {label}
    </Link>
  );
}
