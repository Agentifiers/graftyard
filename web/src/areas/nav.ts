import type { IconSvgElement } from "@hugeicons/react";

// The navigation every area contributes to. Each file src/areas/<area>/nav.ts exports
// `nav: NavEntry[]`; the layouts read them all from here, so an area adds its tabs and
// menu items without editing a layout. web/scripts/check-nav.mjs fails on a target that is
// not a route.

// Where an entry appears.
export type NavSlot =
  | "repo" // a tab under a repository's header
  | "repo-settings" // a section in a repository's settings sidebar
  | "org" // a tab under an organization's header
  | "org-settings" // a section in an organization's settings sidebar
  | "user-settings" // a section in the signed-in user's settings sidebar
  | "user-menu" // an item in the avatar menu at the top right
  | "admin"; // a section of the site admin console

export type NavEntry = {
  slot: NavSlot;
  label: string;
  // A route path with params such as "/$owner/$repo/actions". The layout fills in the
  // params it has: a repo entry may use $owner and $repo, an org entry $org.
  to: string;
  icon: IconSvgElement;
  // Lower sorts first. The core tabs (Code 0, Issues 10, Pull requests 20) leave room.
  order: number;
  // Settings sidebars and the user menu draw a heading per group, in first-seen order.
  group?: string;
  // Extra route paths that should also mark this entry active, such as a detail page.
  match?: string[];
};

const modules = import.meta.glob<{ nav: NavEntry[] }>("./*/nav.ts", { eager: true });

const all = Object.values(modules)
  .flatMap((m) => m.nav)
  .sort((a, b) => a.order - b.order || a.label.localeCompare(b.label));

export function navFor(slot: NavSlot): NavEntry[] {
  return all.filter((e) => e.slot === slot);
}

// Entries grouped by `group`, groups in the order of their first entry.
export function navGroups(slot: NavSlot): { group: string | undefined; entries: NavEntry[] }[] {
  const groups = new Map<string | undefined, NavEntry[]>();
  for (const e of navFor(slot)) groups.set(e.group, [...(groups.get(e.group) ?? []), e]);
  return [...groups].map(([group, entries]) => ({ group, entries }));
}
