import type { ComponentType } from "react";
import { Region } from "@/components/region";

// Widgets an area adds to a page another area owns, such as a deployments panel on the
// repository overview or an agent-session card on a pull request. Each file
// src/areas/<area>/slots.tsx exports `slots: SlotEntry[]`; the host page renders
// <Slot name=... props=... /> and never imports the area. Each widget is its own Region,
// so a failing widget shows its own error and nothing else.

type RepoProps = { owner: string; repo: string };

// Every slot and the props its host passes.
export type SlotProps = {
  topbar: {}; // the top bar, between the logo and the avatar menu
  home: {}; // the signed-in home page's right column
  "repo-actions": RepoProps; // buttons at the right of the repository header
  "repo-overview": RepoProps; // the repository overview's main column (file tree, README)
  "repo-sidebar": RepoProps; // the repository overview's right column
  "issue-sidebar": RepoProps & { number: number }; // the issue page's right column
  "pull-sidebar": RepoProps & { number: number; headSha: string }; // under Checks on a PR
};

export type SlotName = keyof SlotProps;

type Entry<K extends SlotName> = {
  slot: K;
  // The Region name: the error title reads "Could not load <region>".
  region: string;
  order: number;
  Component: ComponentType<SlotProps[K]>;
};

export type SlotEntry = { [K in SlotName]: Entry<K> }[SlotName];

// Builds an entry with its props checked against the slot.
export const slot = <K extends SlotName>(entry: Entry<K>): SlotEntry => entry as SlotEntry;

const modules = import.meta.glob<{ slots: SlotEntry[] }>("./*/slots.tsx", { eager: true });
const all = Object.values(modules)
  .flatMap((m) => m.slots)
  .sort((a, b) => a.order - b.order);

export function Slot<K extends SlotName>({
  name,
  props,
  bare,
}: {
  name: K;
  props: SlotProps[K];
  // Inline slots (top bar, header actions) skip the Region skeleton.
  bare?: boolean;
}) {
  return (all.filter((e) => e.slot === name) as unknown as Entry<K>[]).map(
    ({ region, Component }) => (
      <Region key={region} name={region} fallback={bare ? null : undefined}>
        <Component {...props} />
      </Region>
    ),
  );
}
