import { createFileRoute } from "@tanstack/react-router";
import { StubPage } from "@/components/stub-page";

export const Route = createFileRoute("/$owner/$repo/issues/new")({
  component: () => <StubPage area="issues" what="New issue" />,
});
