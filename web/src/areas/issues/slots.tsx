import type { SlotEntry } from "../slots";
import { IssueSidebar } from "./sidebar";

// A type-only import: calling slot() from ../slots at module load runs before that module
// finishes evaluating (it imports this file eagerly), which throws.
export const slots: SlotEntry[] = [
  {
    slot: "issue-sidebar",
    region: "issue sidebar",
    order: 0,
    Component: IssueSidebar,
  } satisfies SlotEntry,
];
