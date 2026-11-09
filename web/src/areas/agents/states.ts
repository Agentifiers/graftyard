import type { TaskState } from "./api";

// Completed, Failed, TimedOut, and Cancelled are final (design/AGENT-LAYER.md, Shape,
// `AgentTask`).
export const FINAL_STATES: ReadonlySet<TaskState> = new Set([
  "completed",
  "failed",
  "timed_out",
  "cancelled",
]);

// A run's UUID shortens to its first 8 characters; an external `<app_id>.<name>` stays whole.
export const shortId = (id: string) => (/^[0-9a-f]{8}-/.test(id) ? id.slice(0, 8) : id);
