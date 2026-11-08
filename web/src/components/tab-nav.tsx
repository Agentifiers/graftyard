import { Link, useMatchRoute } from "@tanstack/react-router";
import { HugeiconsIcon } from "@hugeicons/react";
import { navFor, type NavSlot } from "@/areas/nav";

// The tab strip under a repository or organization header, built from every area's nav.
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
  return (
    <nav className="-mb-px flex gap-1 overflow-x-auto text-sm" aria-label={label}>
      {navFor(slot).map((t) => {
        const exact = t.to === "/$owner/$repo" || t.to === "/$org";
        const active = [t.to, ...(t.match ?? [])].some((to) =>
          Boolean(
            match({ to: to as never, params: params as never, fuzzy: !exact || to !== t.to }),
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
