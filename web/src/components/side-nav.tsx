import type { ReactNode } from "react";
import { Link, useMatchRoute } from "@tanstack/react-router";
import { HugeiconsIcon } from "@hugeicons/react";
import { navGroups, type NavSlot } from "@/areas/nav";

// A settings or admin sidebar beside its pages, built from every area's nav entries for
// the slot. An entry is active on its own page and every page under it.
export function SideNavLayout({
  slot,
  params,
  label,
  children,
}: {
  slot: Extract<NavSlot, "repo-settings" | "org-settings" | "user-settings" | "admin">;
  params: Record<string, string>;
  label: string;
  children: ReactNode;
}) {
  const match = useMatchRoute();
  const groups = navGroups(slot);
  return (
    <div className="grid items-start gap-6 md:grid-cols-[13rem_minmax(0,1fr)]">
      <nav aria-label={label} className="flex flex-col gap-4 text-sm md:sticky md:top-4">
        {groups.map(({ group, entries }) => (
          <div key={group ?? ""} className="flex flex-col gap-0.5">
            {group && (
              <h2 className="text-muted-foreground px-2 pb-1 text-xs font-medium">{group}</h2>
            )}
            {entries.map((e) => {
              const active = [e.to, ...(e.match ?? [])].some((to) =>
                Boolean(match({ to: to as never, params: params as never, fuzzy: true })),
              );
              return (
                <Link
                  key={e.to}
                  to={e.to as never}
                  params={params as never}
                  data-active={active || undefined}
                  className="text-muted-foreground hover:bg-muted hover:text-foreground data-active:bg-muted data-active:text-foreground data-active:border-primary flex items-center gap-2 rounded-sm border-l-2 border-transparent px-2 py-1.5 font-medium"
                >
                  <HugeiconsIcon icon={e.icon} strokeWidth={2} className="size-4" />
                  {e.label}
                </Link>
              );
            })}
          </div>
        ))}
      </nav>
      <div className="min-w-0">{children}</div>
    </div>
  );
}
