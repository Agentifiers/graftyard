import { createFileRoute } from "@tanstack/react-router";
import { StubPage } from "@/components/stub-page";

export const Route = createFileRoute("/$owner/")({
  component: () => <StubPage area="orgs" what="This profile" />,
});
