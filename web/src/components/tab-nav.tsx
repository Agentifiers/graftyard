import { Link, useMatchRoute } from "@tanstack/react-router";
import { HugeiconsIcon } from "@hugeicons/react";
import { navFor, type NavSlot } from "@/areas/nav";

// The tab strip under a repository or organization header, built from every area's nav.
// The first tab is the overview, so it is active only on its own page.
export function TabNav({
  slot,
  params,
  label,
}: {
  slot: Extract<NavSlot, "repo" | "org">;
  params: Record<string, string>;
  label: string;
}) {
  const match = useMatchRoute();
  const tabs = navFor(slot);
  return (
    <nav className="-mb-px flex gap-1 overflow-x-auto text-sm" aria-label={label}>
      {tabs.map((t, i) => {
        const active = [t.to, ...(t.match ?? [])].some((to) =>
          Boolean(
            match({
              to: to as never,
              params: params as never,
              fuzzy: !(i === 0 && to === t.to),
            }),
          ),
        );
        return (
          <Link
            key={t.to}
            to={t.to as never}
            params={params as never}
            data-active={active || undefined}
            className="text-muted-foreground hover:text-foreground data-active:border-primary data-active:text-foreground flex shrink-0 items-center gap-1.5 border-b-2 border-transparent px-3 py-2 font-medium"
          >
            <HugeiconsIcon icon={t.icon} strokeWidth={2} className="size-4" />
            {t.label}
          </Link>
        );
      })}
    </nav>
  );
}
