import { createFileRoute } from "@tanstack/react-router";
import { StubPage } from "@/components/stub-page";

export const Route = createFileRoute("/search")({
  component: () => <StubPage area="activity" what="Search" />,
});
