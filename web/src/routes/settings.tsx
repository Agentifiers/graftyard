import { createFileRoute, Outlet } from "@tanstack/react-router";
import { SideNavLayout } from "@/components/side-nav";

// The signed-in user's settings. Each area adds sections through "user-settings" nav.
export const Route = createFileRoute("/settings")({
  component: () => (
    <main className="mx-auto max-w-6xl p-4 sm:p-6">
      <h1 className="mb-4 text-xl font-semibold">Settings</h1>
      <SideNavLayout slot="user-settings" params={{}} label="Settings">
        <Outlet />
      </SideNavLayout>
    </main>
  ),
});
