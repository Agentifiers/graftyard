import { createFileRoute } from "@tanstack/react-router";
import { StubPage } from "@/components/stub-page";

export const Route = createFileRoute("/$owner/$repo/releases/")({
  component: () => <StubPage area="releases" what="Releases" />,
});
