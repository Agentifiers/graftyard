// `validate`'s agent outputs (design/DESIGN.md, Shape): `ProtectedHits`, `ChangedPaths`, and `Provenance`, from the
// facts the git cache (gitd, real git) reports for one staged pack. The diff itself is git's; this file only applies the
// design's rules to its output.

import type { Authz, ChangedPaths, Fold, Principal, ProtectedHits, Provenance, RefUpdate, UpdateFacts } from "./model.ts";
import { NOTES_REF, PATHS_MAX, PROTECTED_PATHS } from "./model.ts";
import { globMatch } from "./glob.ts";
import { isAgentPrefix } from "./decide.ts";
import { provenanceOf, type NoteEntry, type RawCommit } from "./provenance.ts";

export interface GitFact { name: string; missing: boolean; ff: boolean; type: string; mergeBase: string; paths: string[]; contains?: string[];
                           notes?: { path: string; mode: string; type: string; oid: string; bytes?: string }[] }
export interface Inspect { facts: GitFact[]; commits: { oid: string; raw: string }[] }

const unb64 = (s: string): Uint8Array => Uint8Array.from(atob(s), (c) => c.charCodeAt(0));

/** Every changed path, or past PATHS_MAX their directories with `truncated` set. */
export function changedOf(refname: string, paths: string[]): ChangedPaths {
  const sorted = [...new Set(paths)].sort();
  if (sorted.length <= PATHS_MAX) return { refname, paths: sorted, truncated: false };
  let depth = 8;
  let dirs: string[] = [];
  for (; depth >= 1; depth--) {
    dirs = [...new Set(sorted.map((p) => { const s = p.split("/"); return s.length > depth ? s.slice(0, depth).join("/") + "/" : p; }))];
    if (dirs.length <= PATHS_MAX) break;
  }
  return { refname, paths: dirs.slice(0, PATHS_MAX), truncated: true };
}

export function protectedOf(refname: string, paths: string[], extra: string[]): ProtectedHits | null {
  const globs = [...PROTECTED_PATHS, ...extra];
  const hits = paths.filter((p) => globs.some((g) => globMatch(g, p)));
  return hits.length ? { refname, paths: hits.sort() } : null;
}

export interface Validated { changed: ChangedPaths[]; protected: ProtectedHits[]; provenance: Provenance[]; facts: UpdateFacts[];
                             parseErrors: number; parseFailed: boolean }

/** `policyFold` holds `AgentPolicy` (the base's in fork mode). */
export function validateFrom(ins: Inspect, updates: RefUpdate[], actor: Principal, policyFold: Fold, authz: Authz): Validated {
  const changed: ChangedPaths[] = [], prot: ProtectedHits[] = [], facts: UpdateFacts[] = [];
  for (const u of updates) {
    const f = ins.facts.find((x) => x.name === u.name);
    facts.push({ name: u.name, ff: f?.ff ?? true, missing: f?.missing ?? false, contains: f?.contains ?? [] });
    if (!f || u.new == null) continue;
    if (actor.kind.t !== "Agent" && !isAgentPrefix(u.name, authz)) continue;
    changed.push(changedOf(u.name, f.paths));
    const h = protectedOf(u.name, f.paths, policyFold.policy.extraPaths);
    if (h) prot.push(h);
  }
  // Provenance: a parse failure leaves it empty and never fails the push.
  let provenance: Provenance[] = [], parseErrors = 0, parseFailed = false;
  try {
    const commits: RawCommit[] = ins.commits.map((c) => ({ oid: c.oid, raw: unb64(c.raw) }));
    const notes: NoteEntry[] = [];
    if (updates.some((u) => u.name === NOTES_REF && u.new)) {
      for (const n of ins.facts.find((x) => x.name === NOTES_REF)?.notes ?? [])
        notes.push({ path: n.path, mode: n.mode, type: n.type, oid: n.oid, bytes: n.bytes ? unb64(n.bytes) : null });
    }
    const r = provenanceOf(commits, notes);
    provenance = r.list;
    parseErrors = r.parseErrors;
  } catch { provenance = []; parseFailed = true; }
  return { changed, protected: prot, provenance, facts, parseErrors, parseFailed };
}
