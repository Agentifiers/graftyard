import { createFileRoute } from "@tanstack/react-router";
import { toFirstSection } from "@/lib/first-section";

export const Route = createFileRoute("/settings/")({
  beforeLoad: () => toFirstSection("user-settings", {}),
});
