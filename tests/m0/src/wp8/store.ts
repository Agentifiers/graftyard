// R2 keys and create-only placement for the M0-WP8 prototype (design/DESIGN.md, Shape, "Packs": `R2Key`,
// `PackStore::place`, `Archived`, `Committed`). Used by the members and the edge.

import { sha256hex } from "./sha256.ts";
import { SITES, type Entry, type GitObj, type ObjType, type SiteKey, type StoredPack } from "./types.ts";

export const ZERO = "0".repeat(64);
/** An entry's digest chains `prev` (design/DESIGN.md, `LogEntry`); members, archives, and the edge's audit use it. */
export const digestOf = (e: Omit<Entry, "digest">) => sha256hex(`${e.prev}|${e.term}|${e.index}|${e.at}|${JSON.stringify(e.body)}`);

export interface Buckets { BUCKET_A: R2Bucket; BUCKET_B: R2Bucket; BUCKET_C: R2Bucket }
export const bucket = (env: Buckets, s: SiteKey): R2Bucket => env[`BUCKET_${s.toUpperCase()}` as "BUCKET_A"];

/** `R2Key` as strings. A pack's key carries its chain because one repository chain is the network here. */
export const R2 = {
  pack: (chain: string, id: string) => `pack/${chain}/${id}`,
  committed: (chain: string, id: string) => `committed/${chain}/${id}`,
  archive: (chain: string, from: number, to: number, digest: string) => `archive/${chain}/${from}-${to}-${digest}`,
  snapshot: (chain: string, to: number, digest: string) => `snapshot/${chain}/core/0-${to}-${digest}`,
};

export async function sha256(body: string | ArrayBuffer): Promise<string> {
  const bytes = typeof body === "string" ? new TextEncoder().encode(body) : body;
  return [...new Uint8Array(await crypto.subtle.digest("SHA-256", bytes))].map((b) => b.toString(16).padStart(2, "0")).join("");
}
const hexOf = (b: ArrayBuffer | undefined) => (b ? [...new Uint8Array(b)].map((x) => x.toString(16).padStart(2, "0")).join("") : "");

/** One create-only put (If-None-Match: *) with R2 checking the SHA-256. A put refused because the key exists counts as
 *  placed only if the existing object has the same SHA-256: archive and snapshot keys are content-addressed, so a
 *  deposed leader's put of the same range is the same bytes. */
export async function putOnce(b: R2Bucket, key: string, body: string, digest: string, meta: Record<string, string>):
  Promise<"put" | "existed" | "conflict"> {
  const r = await b.put(key, body, { onlyIf: new Headers({ "if-none-match": "*" }), sha256: digest, customMetadata: meta });
  if (r) return "put";
  const h = await b.head(key);
  return h && hexOf(h.checksums.sha256) === digest ? "existed" : "conflict";
}

/** HEAD in each site's bucket; true only where the object is present with the expected SHA-256. */
export async function headIn(env: Buckets, sites: SiteKey[], key: string, digest: string): Promise<Record<string, boolean>> {
  const out: Record<string, boolean> = {};
  await Promise.all(sites.map(async (s) => {
    const h = await bucket(env, s).head(key);
    out[s] = !!h && hexOf(h.checksums.sha256) === digest;
  }));
  return out;
}

/** GET from the first site that has it with the right digest; `null` if none does. */
export async function getVerified(env: Buckets, sites: SiteKey[], key: string, digest: string): Promise<{ text: string; site: SiteKey } | null> {
  for (const s of sites) {
    const o = await bucket(env, s).get(key);
    if (!o) continue;
    const text = await o.text();
    if ((await sha256(text)) === digest) return { text, site: s };
  }
  return null;
}

/** `PackStore::place`: create-only puts of one pack file to every site's bucket at once; durable once two succeed. */
export async function placePack(env: Buckets, chain: string, pf: PackFile, rid: string, gc?: "geo" | "cruft"): Promise<StoredPack> {
  const text = JSON.stringify(pf), checksum = await sha256(text);
  const res = await Promise.allSettled(SITES.map((s) => putOnce(bucket(env, s), R2.pack(chain, pf.id), text, checksum, { chain, rid })));
  const sites = SITES.filter((_, i) => res[i].status === "fulfilled" && (res[i] as PromiseFulfilledResult<string>).value !== "conflict");
  if (sites.length < 2) throw new Error(`pack ${pf.id} reached ${sites.length} buckets`);
  return { id: pf.id, checksum, bytes: text.length, sites, ...(gc ? { gc } : {}) };
}

/** The create-only `Committed` marker for a stored pack, in each bucket that holds it. */
export async function markCommitted(env: Buckets, chain: string, p: StoredPack, index: number): Promise<void> {
  const text = JSON.stringify({ chain, index });
  const d = await sha256(text);
  await Promise.all(p.sites.map((s) => bucket(env, s).put(R2.committed(chain, p.id), text, { onlyIf: new Headers({ "if-none-match": "*" }), sha256: d })));
}

/** Reads a stored pack from the first of its sites whose bytes match its checksum. */
export async function readPack(env: Buckets, chain: string, p: StoredPack): Promise<PackFile | null> {
  const r = await getVerified(env, p.sites, R2.pack(chain, p.id), p.checksum);
  return r && (JSON.parse(r.text) as PackFile);
}

/** A git object's SHA-1 over "type size\0body", as git computes it; `true` when it matches `oid`. */
export async function objectOk(o: GitObj): Promise<boolean> {
  const body = Uint8Array.from(atob(o.data), (c) => c.charCodeAt(0));
  const head = new TextEncoder().encode(`${o.type} ${body.length}\0`);
  const all = new Uint8Array(head.length + body.length);
  all.set(head); all.set(body, head.length);
  const h = [...new Uint8Array(await crypto.subtle.digest("SHA-1", all))].map((b) => b.toString(16).padStart(2, "0")).join("");
  return h === o.oid;
}

/** The fold's live pack set: stored packs not consumed, and inline packs after the last `Promoted`. */
export interface PackView { stored: StoredPack[]; inline: GitObj[] }
export type Obj = GitObj & { since?: number };

/** Every object of the live packs, keyed by oid, with stored packs read from R2 and their checksums verified. */
export async function loadPacks(env: Buckets, chain: string, view: PackView): Promise<{ objs: Map<string, Obj>; unreadable: string[]; reads: number }> {
  const objs = new Map<string, Obj>(), unreadable: string[] = [];
  const files = await Promise.all(view.stored.map(async (p) => ({ p, f: await readPack(env, chain, p).catch(() => null) })));
  // `since` is the earliest time any cruft pack records for the object (git's cruft mtimes); a copy in a normal pack
  // carries none.
  const add = (o: Obj) => {
    const had = objs.get(o.oid);
    if (!had) { objs.set(o.oid, { ...o }); return; }
    if (o.since !== undefined) had.since = Math.min(had.since ?? Infinity, o.since);
  };
  for (const { p, f } of files) {
    if (!f) { unreadable.push(p.id); continue; }
    for (const o of f.objects) add(o);
  }
  for (const o of view.inline) add(o);
  return { objs, unreadable, reads: files.length };
}

/** An archive object's bytes. Members rebuild it from their own log rows, so it must serialize the same everywhere. */
export const archiveText = (chain: string, from: number, to: number, entries: Entry[]) => JSON.stringify({ chain, from, to, entries });

/** Everything reachable from `roots` in `objs`; oids it cannot find are `missing`. */
export function closure(objs: Map<string, Obj>, roots: string[]): { reached: Obj[]; missing: string[] } {
  const seen = new Set<string>(), reached: Obj[] = [], missing: string[] = [];
  const stack = [...roots];
  while (stack.length) {
    const oid = stack.pop()!;
    if (seen.has(oid)) continue;
    seen.add(oid);
    const o = objs.get(oid);
    if (!o) { missing.push(oid); continue; }
    reached.push(o);
    for (const l of o.links) if (!seen.has(l)) stack.push(l);
  }
  return { reached, missing };
}

/** Pack ids sort by calendar time, like the design's ULID `PackId`: 11 base-36 digits of ms, then randomness. */
export const packId = (calMs: number) => `${calMs.toString(36).padStart(11, "0")}-${crypto.randomUUID().slice(0, 8)}`;
export const packTime = (id: string) => parseInt(id.slice(0, 11), 36);

/** A pack object in R2: real git objects (type and raw body), each with `since` in a cruft pack (git's cruft mtimes). */
export interface PackFile { id: string; cruft: boolean; objects: (GitObj & { since?: number })[] }

/** The oids a git object names: a commit's tree and parents, a tag's object, a tree's entries except gitlinks. */
export function linksOf(type: ObjType, data: string): string[] {
  const bin = atob(data);
  if (type === "commit" || type === "tag") {
    const end = bin.indexOf("\n\n");
    const out: string[] = [];
    for (const line of bin.slice(0, end < 0 ? bin.length : end).split("\n")) {
      const m = /^(tree|parent|object) ([0-9a-f]{40})$/.exec(line);
      if (m) out.push(m[2]);
    }
    return out;
  }
  if (type === "tree") {
    const out: string[] = [];
    let i = 0;
    while (i < bin.length) {
      const sp = bin.indexOf(" ", i), nul = bin.indexOf("\0", sp);
      const mode = bin.slice(i, sp);
      let hex = "";
      for (let j = nul + 1; j < nul + 21; j++) hex += bin.charCodeAt(j).toString(16).padStart(2, "0");
      if (mode !== "160000") out.push(hex);
      i = nul + 21;
    }
    return out;
  }
  return [];
}
