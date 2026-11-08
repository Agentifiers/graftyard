import { createFileRoute } from "@tanstack/react-router";
import { StubPage } from "@/components/stub-page";

export const Route = createFileRoute("/new")({
  component: () => <StubPage area="repo-admin" what="New repository" />,
});
