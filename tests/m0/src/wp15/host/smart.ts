// Smart HTTP framing for M0-WP10: pkt-lines, receive-pack requests and report-status, and the ref advertisements with
// SESSION_REFS rules (design/DESIGN.md, Shape, `SESSION_REFS`: "Never in v0 or v1 advertisement, and listed by v2
// `ls-refs` only to a `ref-prefix` that starts with it"). Pure functions over bytes and the fold's refs.

import { HOST_REFS, SESSION_REFS, ZERO } from "./model.ts";
import type { RefUpdate } from "./model.ts";

const enc = new TextEncoder();
const dec = new TextDecoder();

export function pkt(s: string | Uint8Array): Uint8Array {
  const b = typeof s === "string" ? enc.encode(s) : s;
  const out = new Uint8Array(b.length + 4);
  out.set(enc.encode((b.length + 4).toString(16).padStart(4, "0")));
  out.set(b, 4);
  return out;
}
export const FLUSH = enc.encode("0000");
export const DELIM = enc.encode("0001");

export function concat(parts: Uint8Array[]): Uint8Array {
  const n = parts.reduce((a, p) => a + p.length, 0);
  const out = new Uint8Array(n);
  let o = 0;
  for (const p of parts) { out.set(p, o); o += p.length; }
  return out;
}

/** Reads pkt-lines from `buf` at `off`. Returns null for flush, "" for delim, else the payload. */
export function readPkt(buf: Uint8Array, off: number): { line: Uint8Array | null; delim: boolean; next: number } {
  const len = parseInt(dec.decode(buf.subarray(off, off + 4)), 16);
  if (Number.isNaN(len)) throw new Error(`bad pkt-line at ${off}`);
  if (len === 0) return { line: null, delim: false, next: off + 4 };
  if (len === 1) return { line: null, delim: true, next: off + 4 };
  if (len < 4) throw new Error(`bad pkt-line length ${len}`);
  return { line: buf.subarray(off + 4, off + len), delim: false, next: off + len };
}

export interface ReceiveRequest { updates: RefUpdate[]; caps: Set<string>; pack: Uint8Array; options: string[] }

export function parseReceive(body: Uint8Array): ReceiveRequest {
  const updates: RefUpdate[] = [];
  let caps = new Set<string>();
  let off = 0;
  for (;;) {
    const p = readPkt(body, off);
    off = p.next;
    if (!p.line) break;
    let s = dec.decode(p.line).replace(/\n$/, "");
    const nul = s.indexOf("\0");
    if (nul >= 0) { caps = new Set(s.slice(nul + 1).trim().split(" ").filter(Boolean)); s = s.slice(0, nul); }
    const [o, n, name] = s.split(" ");
    updates.push({ name, old: o === ZERO ? null : o, new: n === ZERO ? null : n });
  }
  const options: string[] = [];
  if (caps.has("push-options")) {
    for (;;) {
      const p = readPkt(body, off);
      off = p.next;
      if (!p.line) break;
      options.push(dec.decode(p.line).replace(/\n$/, ""));
    }
  }
  return { updates, caps, pack: body.subarray(off), options };
}

/** report-status, wrapped in side-band-64k band 1 when the client asked, with `remote:` lines on band 2. */
export function reportStatus(unpack: string, refs: { name: string; ok: boolean; text?: string }[], sideband: boolean,
                             remote: string[]): Uint8Array {
  const rs = concat([pkt(`unpack ${unpack}\n`), ...refs.map((r) => pkt(r.ok ? `ok ${r.name}\n` : `ng ${r.name} ${r.text}\n`)), FLUSH]);
  if (!sideband) return rs;
  const parts: Uint8Array[] = [];
  for (const m of remote) parts.push(pkt(concat([new Uint8Array([2]), enc.encode(m + "\n")])));
  for (let i = 0; i < rs.length; i += 65515) parts.push(pkt(concat([new Uint8Array([1]), rs.subarray(i, i + 65515)])));
  parts.push(FLUSH);
  return concat(parts);
}

export interface RefRow { name: string; oid: string; peeled?: string }

/** v0 and v1 advertisements never carry anything under `refs/graftyard/`. */
export const advertisedV01 = (refs: RefRow[]): RefRow[] => refs.filter((r) => !r.name.startsWith(HOST_REFS));

const UPLOAD_CAPS = "multi_ack thin-pack side-band side-band-64k ofs-delta shallow deepen-since deepen-not deepen-relative " +
  "no-progress include-tag multi_ack_detailed no-done filter object-format=sha1 agent=graftyard-m0";
const RECEIVE_CAPS = "report-status delete-refs side-band-64k quiet atomic ofs-delta object-format=sha1 agent=graftyard-m0";

export function advertise(service: "git-upload-pack" | "git-receive-pack", proto: 0 | 1 | 2, refs: RefRow[], head: string | null): Uint8Array {
  if (service === "git-upload-pack" && proto === 2) {
    return concat([pkt("version 2\n"), pkt("agent=graftyard-m0\n"), pkt("ls-refs=unborn\n"), pkt("fetch=shallow wait-for-done filter\n"),
      pkt("server-option\n"), pkt("object-format=sha1\n"), FLUSH]);
  }
  const parts: Uint8Array[] = [pkt(`# service=${service}\n`), FLUSH];
  if (proto === 1) parts.push(pkt("version 1\n"));
  const shown = advertisedV01(refs).sort((a, b) => a.name < b.name ? -1 : 1);
  let caps = service === "git-upload-pack" ? UPLOAD_CAPS : RECEIVE_CAPS;
  const headRow = head ? shown.find((r) => r.name === head) : undefined;
  const lines: string[] = [];
  if (service === "git-upload-pack" && headRow) { caps = `${caps} symref=HEAD:${head}`; lines.push(`${headRow.oid} HEAD`); }
  for (const r of shown) {
    lines.push(`${r.oid} ${r.name}`);
    if (service === "git-upload-pack" && r.peeled) lines.push(`${r.peeled} ${r.name}^{}`);
  }
  if (!lines.length) lines.push(`${ZERO} capabilities^{}`);
  lines.forEach((l, i) => parts.push(pkt(i === 0 ? `${l}\0${caps}\n` : `${l}\n`)));
  parts.push(FLUSH);
  return concat(parts);
}

export interface LsRefsArgs { symrefs: boolean; peel: boolean; unborn: boolean; prefixes: string[] }

export function parseCommandRequest(body: Uint8Array): { command: string; caps: string[]; args: string[] } {
  let off = 0, command = "";
  const caps: string[] = [], args: string[] = [];
  let inArgs = false;
  while (off < body.length) {
    const p = readPkt(body, off);
    off = p.next;
    if (p.delim) { inArgs = true; continue; }
    if (!p.line) break;
    const s = dec.decode(p.line).replace(/\n$/, "");
    if (!inArgs && s.startsWith("command=")) command = s.slice(8);
    else if (!inArgs) caps.push(s);
    else args.push(s);
  }
  return { command, caps, args };
}

/** Whether `name` is listed to a v2 `ls-refs` with these prefixes. Ordinary refs follow git's prefix rule (any prefix
 *  matches, or no prefixes at all). A ref under `refs/graftyard/` is listed only when it is under SESSION_REFS and a
 *  prefix that itself starts with SESSION_REFS matches it. */
export function lsRefsVisible(name: string, prefixes: string[]): boolean {
  if (name.startsWith(HOST_REFS)) {
    if (!name.startsWith(SESSION_REFS)) return false;
    return prefixes.some((p) => p.startsWith(SESSION_REFS) && name.startsWith(p));
  }
  return prefixes.length === 0 || prefixes.some((p) => name.startsWith(p));
}

export function lsRefs(args: string[], refs: RefRow[], head: string | null): Uint8Array {
  const a: LsRefsArgs = { symrefs: false, peel: false, unborn: false, prefixes: [] };
  for (const s of args) {
    if (s === "symrefs") a.symrefs = true;
    else if (s === "peel") a.peel = true;
    else if (s === "unborn") a.unborn = true;
    else if (s.startsWith("ref-prefix ")) a.prefixes.push(s.slice(11));
  }
  const out: Uint8Array[] = [];
  const headRow = head ? refs.find((r) => r.name === head) : undefined;
  if (lsRefsVisible("HEAD", a.prefixes) && head) {
    if (headRow) out.push(pkt(`${headRow.oid} HEAD${a.symrefs ? ` symref-target:${head}` : ""}\n`));
    else if (a.unborn) out.push(pkt(`unborn HEAD${a.symrefs ? ` symref-target:${head}` : ""}\n`));
  }
  for (const r of [...refs].sort((x, y) => x.name < y.name ? -1 : 1)) {
    if (!lsRefsVisible(r.name, a.prefixes)) continue;
    out.push(pkt(`${r.oid} ${r.name}${a.peel && r.peeled ? ` peeled:${r.peeled}` : ""}\n`));
  }
  out.push(FLUSH);
  return concat(out);
}

export async function readBody(req: Request): Promise<Uint8Array> {
  let stream = req.body;
  if (!stream) return new Uint8Array(0);
  if ((req.headers.get("content-encoding") ?? "").includes("gzip")) stream = stream.pipeThrough(new DecompressionStream("gzip"));
  return new Uint8Array(await new Response(stream).arrayBuffer());
}
