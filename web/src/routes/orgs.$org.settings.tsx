import { createFileRoute, Outlet } from "@tanstack/react-router";
import { SideNavLayout } from "@/components/side-nav";

// An organization's settings. Each area adds sections through "org-settings" nav.
export const Route = createFileRoute("/orgs/$org/settings")({
  component: OrgSettings,
});

function OrgSettings() {
  const params = Route.useParams();
  return (
    <SideNavLayout slot="org-settings" params={params} label="Organization settings">
      <Outlet />
    </SideNavLayout>
  );
}
