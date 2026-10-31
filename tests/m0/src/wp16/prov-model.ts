// The provenance types of src/agent/model.ts (M0-WP10), copied for M0-WP16 (design/AGENT-LAYER.md, Shape, `Provenance`).
export type Oid = string;
export type TrailerKey = "GraftyardSession" | "ClaudeSession" | "AmpThreadId" | "CoAuthoredBy" | "AssistedBy";
export type Source = { t: "Claimed"; key: TrailerKey; value: string } | { t: "AiNote"; note: Oid } | { t: "Session"; session: string };
export interface Provenance { commit: Oid; sources: Source[] }
