import { createFileRoute } from "@tanstack/react-router";
import { StubPage } from "@/components/stub-page";

export const Route = createFileRoute("/orgs/$org/teams/$team")({
  component: () => <StubPage area="orgs" what="This team" />,
});
