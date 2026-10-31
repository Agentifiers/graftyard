// `validate`'s `Provenance` output (design/DESIGN.md, Shape): the new commits' trailers and any `refs/notes/ai` update
// in the same push. "A parse failure leaves it empty and never fails the push": every entry point here catches and
// returns an empty result, and the inputs are raw bytes, so no encoding, size, or shape can throw past it.

import type { Provenance, Source, TrailerKey } from "./model.ts";

/** Recognized trailer keys, matched case-insensitively as git does. Only these become `Source::Claimed`. */
const KEYS: Record<string, TrailerKey> = {
  "graftyard-session": "GraftyardSession", "claude-session": "ClaudeSession", "amp-thread-id": "AmpThreadId",
  "co-authored-by": "CoAuthoredBy", "assisted-by": "AssistedBy",
};
/** Only these class a push (`TrailerKey` doc comment). CoAuthoredBy is recorded, never classes. */
export const CLASSING: readonly TrailerKey[] = ["GraftyardSession", "ClaudeSession", "AmpThreadId", "AssistedBy"];

export const VALUE_MAX = 1024;          // bytes kept per trailer value [prototype cap]
export const TRAILERS_MAX = 32;         // recognized trailers kept per commit [prototype cap]
export const MESSAGE_SCAN_MAX = 1 << 20; // only the last 1 MiB of a message is scanned for its trailer block
export const NOTE_MAX = 64 << 10;

const dec = new TextDecoder("utf-8", { fatal: false, ignoreBOM: false });
const TOKEN = /^([A-Za-z0-9][A-Za-z0-9-]*)[ \t]*:[ \t]*(.*)$/;
// Control and invisible/bidi characters dropped from values: C0 (but not tab), DEL, C1, zero-width, bidi overrides.
const STRIP = /[\u0000-\u0008\u000a-\u001f\u007f-\u009f\u200b-\u200f\u202a-\u202e\u2060-\u2064\u2066-\u2069\ufeff]/g;

function clean(v: string): string {
  let s = v.replace(STRIP, "").trim();
  const b = new TextEncoder().encode(s);
  if (b.length > VALUE_MAX) s = dec.decode(b.subarray(0, VALUE_MAX)).replace(/\ufffd+$/, "");
  return s;
}

/** The message of a raw commit object: everything after the first blank line that ends the header. */
export function commitMessage(raw: Uint8Array): string {
  let i = 0;
  for (; i + 1 < raw.length; i++) if (raw[i] === 10 && raw[i + 1] === 10) break;
  const body = i + 1 < raw.length ? raw.subarray(i + 2) : new Uint8Array(0);
  const tail = body.length > MESSAGE_SCAN_MAX ? body.subarray(body.length - MESSAGE_SCAN_MAX) : body;
  return dec.decode(tail);
}

/** git's trailer-block rule, simplified: the last paragraph, where every line is `Token: value` or a continuation, or at
 *  least 25% are trailers and one of them is a recognized key. Returns the recognized trailers only. */
export function parseTrailers(message: string): { key: TrailerKey; value: string }[] {
  const lines = message.replace(/\r\n?/g, "\n").replace(/\n+$/, "").split("\n");
  // Strip a trailing comment/cut-line-free tail; git also ignores lines starting with '#' in --cleanup, we keep them.
  let start = lines.length;
  while (start > 0 && lines[start - 1].trim() !== "") start--;
  if (start === 0) return []; // the whole message is one paragraph: the subject. git needs a blank line before the block.
  const block = lines.slice(start);
  const items: { token: string; value: string }[] = [];
  let trailers = 0, other = 0, recognized = false;
  for (const line of block) {
    if (/^[ \t]/.test(line) && items.length) { items[items.length - 1].value += " " + line.trim(); continue; }
    const m = TOKEN.exec(line);
    if (m) {
      trailers++;
      items.push({ token: m[1], value: m[2] });
      if (KEYS[m[1].toLowerCase()]) recognized = true;
    } else other++;
  }
  if (trailers === 0) return [];
  if (other > 0 && !(recognized && trailers * 4 >= trailers + other)) return [];
  const out: { key: TrailerKey; value: string }[] = [];
  for (const it of items) {
    const key = KEYS[it.token.toLowerCase()];
    if (!key) continue;
    out.push({ key, value: clean(it.value) });
    if (out.length >= TRAILERS_MAX) break;
  }
  return out;
}

export interface RawCommit { oid: string; raw: Uint8Array }
/** A changed entry of the `refs/notes/ai` tree: `path` is the annotated object id with fan-out slashes. */
export interface NoteEntry { path: string; mode: string; type: string; oid: string; bytes: Uint8Array | null }

const HEX40 = /^[0-9a-f]{40}$/;

/** Never throws. Commits whose trailers fail to parse contribute no sources; malformed notes are skipped. */
export function provenanceOf(commits: RawCommit[], notes: NoteEntry[]): { list: Provenance[]; parseErrors: number } {
  const byCommit = new Map<string, Source[]>();
  let parseErrors = 0;
  for (const c of commits) {
    try {
      const t = parseTrailers(commitMessage(c.raw));
      if (t.length) byCommit.set(c.oid, t.map((x) => ({ t: "Claimed", key: x.key, value: x.value })));
    } catch { parseErrors++; }
  }
  for (const n of notes) {
    try {
      const target = n.path.replace(/\//g, "").toLowerCase();
      if (!HEX40.test(target) || n.type !== "blob") { parseErrors++; continue; }
      const s = byCommit.get(target) ?? [];
      s.push({ t: "AiNote", note: n.oid });
      byCommit.set(target, s);
    } catch { parseErrors++; }
  }
  return { list: [...byCommit].map(([commit, sources]) => ({ commit, sources })), parseErrors };
}

/** True when any source would class an App token's push as Agent (`TrailerKey`). */
export const classes = (p: Provenance[]): boolean =>
  p.some((x) => x.sources.some((s) => s.t === "Claimed" && CLASSING.includes(s.key)));

/** The session a `Graftyard-Session` value names, if it is `https://<host>/<owner>/<repo>/_graftyard/sessions/<id>`
 *  with the install's host and the given repository. Anything else is null and stays `Claimed`. */
export function sessionFromUrl(value: string, host: string, repo: string): string | null {
  try {
    const u = new URL(value);
    if (u.protocol !== "https:" || u.host.toLowerCase() !== host.toLowerCase() || u.search || u.hash || u.username || u.password) return null;
    const m = /^\/([^/]+)\/([^/]+)\/_graftyard\/sessions\/([^/]+)$/.exec(u.pathname);
    if (!m || `${m[1]}/${m[2]}`.toLowerCase() !== repo.toLowerCase()) return null;
    return decodeURIComponent(m[3]);
  } catch { return null; }
}
