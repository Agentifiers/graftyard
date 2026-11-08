import { createFileRoute } from "@tanstack/react-router";
import { StubPage } from "@/components/stub-page";

export const Route = createFileRoute("/$owner/$repo/compare")({
  component: () => <StubPage area="pulls" what="Compare" />,
});
