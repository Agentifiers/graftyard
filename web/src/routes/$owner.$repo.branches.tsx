import { createFileRoute } from "@tanstack/react-router";
import { StubPage } from "@/components/stub-page";

export const Route = createFileRoute("/$owner/$repo/branches")({
  component: () => <StubPage area="code" what="Branches" />,
});
