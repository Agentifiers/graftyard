import { notFound, redirect } from "@tanstack/react-router";
import { navFor, type NavSlot } from "@/areas/nav";

// A settings or admin index has no page of its own: it sends you to the slot's first
// section, so whichever area sorts first owns the landing page. Search params such as
// ?fail= carry over.
export function toFirstSection(slot: NavSlot, params: Record<string, string>): never {
  const first = navFor(slot)[0];
  if (!first) throw notFound();
  throw redirect({
    to: first.to as never,
    params: params as never,
    search: true as never,
    replace: true,
  });
}
