import { createFileRoute } from "@tanstack/react-router";
import { toFirstSection } from "@/lib/first-section";

export const Route = createFileRoute("/orgs/$org/settings/")({
  beforeLoad: ({ params }) => toFirstSection("org-settings", params),
});
