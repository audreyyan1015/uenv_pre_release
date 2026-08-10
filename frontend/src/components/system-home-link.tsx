import { Link } from "@tanstack/react-router";
import { Network } from "lucide-react";

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
      className={cn(buttonVariants({ variant: "outline", size: "sm" }), className)}
      aria-label="返回系统拓扑"
    >
      <Network className="h-4 w-4" />
      {label}
    </Link>
  );
}
