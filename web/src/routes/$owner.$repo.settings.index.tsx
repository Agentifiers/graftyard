import { createFileRoute } from "@tanstack/react-router";
import { toFirstSection } from "@/lib/first-section";

export const Route = createFileRoute("/$owner/$repo/settings/")({
  beforeLoad: ({ params }) => toFirstSection("repo-settings", params),
});
