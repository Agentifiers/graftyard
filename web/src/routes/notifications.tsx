import { createFileRoute } from "@tanstack/react-router";
import { StubPage } from "@/components/stub-page";

export const Route = createFileRoute("/notifications")({
  component: () => <StubPage area="activity" what="Notifications" />,
});
