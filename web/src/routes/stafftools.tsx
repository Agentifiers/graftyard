import { createFileRoute, Outlet } from "@tanstack/react-router";
import { HugeiconsIcon } from "@hugeicons/react";
import { ShieldUserIcon } from "@hugeicons/core-free-icons";
import { SideNavLayout } from "@/components/side-nav";

// The site admin console. Each area adds sections through "admin" nav.
export const Route = createFileRoute("/stafftools")({
  component: () => (
    <main className="mx-auto max-w-6xl p-4 sm:p-6">
      <h1 className="mb-4 flex items-center gap-2 text-xl font-semibold">
        <HugeiconsIcon icon={ShieldUserIcon} strokeWidth={2} className="size-5" />
        Site admin
      </h1>
      <SideNavLayout slot="admin" params={{}} label="Site admin">
        <Outlet />
      </SideNavLayout>
    </main>
  ),
});
