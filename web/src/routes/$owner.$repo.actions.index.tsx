import { createFileRoute } from "@tanstack/react-router";
import { StubPage } from "@/components/stub-page";

export const Route = createFileRoute("/$owner/$repo/actions/")({
  component: () => <StubPage area="actions" what="Actions" />,
});
