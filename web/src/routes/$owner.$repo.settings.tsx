import { createFileRoute, Outlet } from "@tanstack/react-router";
import { SideNavLayout } from "@/components/side-nav";

// A repository's settings. Each area adds sections through "repo-settings" nav.
export const Route = createFileRoute("/$owner/$repo/settings")({
  component: RepoSettings,
});

function RepoSettings() {
  const params = Route.useParams();
  return (
    <SideNavLayout slot="repo-settings" params={params} label="Repository settings">
      <Outlet />
    </SideNavLayout>
  );
}
