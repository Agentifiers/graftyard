import { createFileRoute } from "@tanstack/react-router";
import { StubPage } from "@/components/stub-page";

export const Route = createFileRoute("/$owner/$repo/_graftyard/sessions/$uuid")({
  component: () => <StubPage area="agents" what="This agent session" />,
});
