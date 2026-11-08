import { createFileRoute } from "@tanstack/react-router";
import { StubPage } from "@/components/stub-page";

export const Route = createFileRoute("/$owner/$repo/tree/$")({
  component: () => <StubPage area="code" what="This directory" />,
});
