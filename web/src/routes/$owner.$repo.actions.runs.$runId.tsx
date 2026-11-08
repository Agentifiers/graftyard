import { createFileRoute } from "@tanstack/react-router";
import { StubPage } from "@/components/stub-page";

export const Route = createFileRoute("/$owner/$repo/actions/runs/$runId")({
  component: () => <StubPage area="actions" what="This workflow run" />,
});
