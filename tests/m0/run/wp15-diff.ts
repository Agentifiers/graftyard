// M0-WP15, L0-11: the byte diff over results/raw/wp15-l011.jsonl (run/wp15-tee.ts). For each recorded request it
// compares the "on" (agent layer) and "off" responses: status, every header, and the body bytes, after masking only
// what the row allows to differ, request ids and times. Masked: the headers `date`, `cf-ray`, `x-github-request-id`,
// `server-timing`, `report-to`, `nel`, `alt-svc`, `x-ratelimit-reset`; JSON values that are ISO times or epoch seconds
// within a day of the request; ids minted per request (`jti`-like `m...`/`u...`/`j...`, UUIDs, gym0_ tokens, ghr_).
// Anything else that differs is listed. Writes results/raw/wp15-l011-diff.json and prints a summary.
//
//   node run/wp15-diff.ts [in.jsonl]

import { readFileSync, writeFileSync } from "node:fs";
import { ROOT } from "./wp15.ts";

const file = process.argv[2] ?? `${ROOT}results/raw/wp15-l011.jsonl`;
const rows = readFileSync(file, "utf8").split("\n").filter(Boolean).map((l) => JSON.parse(l));
const MASK_H = new Set(["date", "cf-ray", "x-github-request-id", "server-timing", "report-to", "nel", "alt-svc", "x-ratelimit-reset", "cf-cache-status"]);
const ISO = /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(\.\d+)?Z$/;
const ID = /^(gym0_[A-Za-z0-9_.-]+|ghr_[0-9a-f]+|[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}|q\d+-[\w-]+)$/;
const OID_SEED = /^[0-9a-f]{40}$/; // commit ids differ between the deployments only through commit times (see below)

function mask(v: unknown, at: number, path: string, oids: Map<string, string>): unknown {
  if (typeof v === "string") {
    if (ISO.test(v) && Math.abs(Date.parse(v) - at) < 86_400_000 * 2) return "<time>";
    if (ID.test(v)) return "<id>";
    if (OID_SEED.test(v) && oids.has(v)) return `<oid:${oids.get(v)}>`;
    return v.replace(/gym0_[A-Za-z0-9_.-]+/g, "<id>").replace(/[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}/g, "<id>");
  }
  if (typeof v === "number" && v > 1.7e9 && v < 2.2e9 && Math.abs(v * 1000 - at) < 86_400_000 * 2 && /reset|_at|time/i.test(path)) return "<time>";
  if (Array.isArray(v)) return v.map((x, i) => mask(x, at, `${path}[${i}]`, oids));
  if (v && typeof v === "object") return Object.fromEntries(Object.entries(v).map(([k, x]) => [k, mask(x, at, `${path}.${k}`, oids)]));
  return v;
}
function walkDiff(a: unknown, b: unknown, path: string, out: string[]): void {
  if (JSON.stringify(a) === JSON.stringify(b)) return;
  if (a && b && typeof a === "object" && typeof b === "object" && !Array.isArray(a) && !Array.isArray(b)) {
    for (const k of new Set([...Object.keys(a), ...Object.keys(b)])) walkDiff((a as any)[k], (b as any)[k], `${path}.${k}`, out);
    return;
  }
  if (Array.isArray(a) && Array.isArray(b) && a.length === b.length) { a.forEach((x, i) => walkDiff(x, b[i], `${path}[${i}]`, out)); return; }
  out.push(`${path}: on=${JSON.stringify(a)?.slice(0, 120)} off=${JSON.stringify(b)?.slice(0, 120)}`);
}

// Seeded commits differ by deployment (each was pushed separately), so pair them by ref name before comparing.
const oids = new Map<string, string>();
for (const r of rows) for (const side of ["on", "off"] as const) {
  const t = r[side].text as string | undefined;
  const m = t && /"ref":"(refs\/[^"]+)","object":\{"sha":"([0-9a-f]{40})"/.exec(t);
  if (m) oids.set(m[2], m[1]);
}

interface Diff { seq: number; client: string; method: string; path: string; diffs: string[] }
const found: Diff[] = [];
let identicalRaw = 0, identicalMasked = 0;
const byClient: Record<string, { n: number; differ: number }> = {};
for (const r of rows) {
  const at = Date.parse(r.at);
  const c = (byClient[r.client] ??= { n: 0, differ: 0 }); c.n++;
  const raw = r.on.status === r.off.status && JSON.stringify(r.on.headers) === JSON.stringify(r.off.headers) && (r.on.text ?? r.on.b64) === (r.off.text ?? r.off.b64);
  if (raw) { identicalRaw++; identicalMasked++; continue; }
  const d: string[] = [];
  if (r.on.status !== r.off.status) d.push(`status: on=${r.on.status} off=${r.off.status}`);
  for (const k of new Set([...Object.keys(r.on.headers), ...Object.keys(r.off.headers)])) {
    if (MASK_H.has(k)) continue;
    const a = r.on.headers[k], b = r.off.headers[k];
    if (a !== b && JSON.stringify(mask(a, at, k, oids)) !== JSON.stringify(mask(b, at, k, oids))) d.push(`header ${k}: on=${a} off=${b}`);
  }
  const parse = (s: { text?: string; b64?: string }) => { if (s.b64) return { __bytes: s.b64 }; try { return JSON.parse(s.text ?? ""); } catch { return { __text: s.text }; } };
  walkDiff(mask(parse(r.on), at, "body", oids), mask(parse(r.off), at, "body", oids), "body", d);
  if (d.length) { found.push({ seq: r.seq, client: r.client, method: r.method, path: r.path, diffs: d }); c.differ++; } else identicalMasked++;
}
// Group by the kind of difference (path with ids collapsed, plus the first diff line).
const kinds: Record<string, { count: number; example: Diff }> = {};
for (const f of found) {
  const k = `${f.method} ${f.path.replace(/\?.*/, "").replace(/\d+/g, "N")} | ${f.diffs[0].replace(/on=.*/, "")}`;
  (kinds[k] ??= { count: 0, example: f }).count++;
}
const summary = { file, requests: rows.length, identicalBytes: identicalRaw, identicalAfterMask: identicalMasked, differing: found.length, byClient, kinds };
writeFileSync(`${ROOT}results/raw/wp15-l011-diff.json`, JSON.stringify({ ...summary, all: found }, null, 2));
console.log(JSON.stringify(summary, null, 2));
