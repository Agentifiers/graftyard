// M0-WP13: the trust view (design/AGENT-LAYER.md, Shape, "Trust view"): `TrustView`, `Hidden`, `AuthorTrust`,
// `HIDDEN_CHARS`, and `AGENT_MEDIA_TYPE`. A pure function of the stored text. `body` is never touched; `visible` is
// computed beside it.
//
// The scanner reads the source the way a GFM renderer would, and where it cannot be sure what a human sees it drops
// more, never less. That makes `visible` a subset of what GitHub's renderer shows (checked against GitHub's own
// renderer by run/agent-l009.ts):
// - Block context, line by line: a line that starts (after container markers `>`, `-`, `*`, `+`, `1.`) with `<` and a
//   tag name, `/`, `!`, or `?` opens raw-HTML context until a blank line, even where CommonMark would not (type 7
//   cannot interrupt a paragraph), because raw HTML hides more than markdown does. A fence (```, ~~~) is code only
//   outside raw-HTML context.
// - Inline, outside code: HTML comments (HTML5 closing rules: `<!-->`, `<!--->`, `-->`, `--!>`), processing
//   instructions, declarations, and CDATA are dropped and counted as `html_comments`. In raw-HTML context an unclosed
//   comment runs to the end of the text; in a paragraph it must close before a blank line or it is literal text.
// - Raw HTML tags keep their name and `href`/`src`, and lose every other attribute. A `title` attribute, a markdown
//   inline link or image title, and a link reference definition's title count as `link_title_text`.
// - `<details>` without an `open` attribute is collapsed: everything inside except its first `<summary>` is dropped
//   and the block counts once in `collapsed`. Opening tags count everywhere, code included, and `</details>` counts
//   only outside code and comments, so the dropped region can only grow.
// - Characters in HIDDEN_CHARS, raw or as a numeric or named character reference outside code, are dropped and each
//   counts in `invisible_chars`. In code a reference is literal text and stays.
// - Link reference definitions (the `[//]: # (...)` comment idiom included) and footnote definitions nothing refers
//   to render as nothing, so they are dropped. One without a title counts in `html_comments`.

export const AGENT_MEDIA_TYPE = "application/vnd.graftyard.agent+json";

export interface Hidden { html_comments: number; invisible_chars: number; link_title_text: number; collapsed: number }
export type AuthorAssociation = "OWNER" | "MEMBER" | "COLLABORATOR" | "CONTRIBUTOR" | "FIRST_TIME_CONTRIBUTOR" | "FIRST_TIMER" | "NONE";
/** `trusted` means a human who can push. Bots and every `AgentApp` are never trusted authors. */
export interface AuthorTrust { association: AuthorAssociation; can_push: boolean; is_bot: boolean; is_agent: boolean;
                               first_contribution: boolean; trusted: boolean }
export interface TrustView { visible: string; hidden: Hidden; author: AuthorTrust }

/** The design's HIDDEN_CHARS, as inclusive code point ranges. */
export const HIDDEN_CHARS: readonly (readonly [number, number])[] = [[0x00ad, 0x00ad], [0x034f, 0x034f], [0x061c, 0x061c],
  [0x115f, 0x1160], [0x17b4, 0x17b5], [0x180e, 0x180e], [0x200b, 0x200f], [0x202a, 0x202e], [0x2060, 0x2064],
  [0x2066, 0x206f], [0x3164, 0x3164], [0xfeff, 0xfeff], [0xffa0, 0xffa0], [0xfff9, 0xfffb], [0xe0000, 0xe007f],
  [0xe0100, 0xe01ef]];
export const isHiddenChar = (cp: number): boolean => HIDDEN_CHARS.some(([a, b]) => cp >= a && cp <= b);

/** Named references (HTML5) that decode into HIDDEN_CHARS. Every other named reference stays as text. */
const HIDDEN_NAMED: Record<string, number> = { shy: 0xad, zwj: 0x200d, zwnj: 0x200c, lrm: 0x200e, rlm: 0x200f,
  ZeroWidthSpace: 0x200b, NegativeVeryThinSpace: 0x200b, NegativeThinSpace: 0x200b, NegativeMediumSpace: 0x200b,
  NegativeThickSpace: 0x200b, NoBreak: 0x2060, ApplyFunction: 0x2061, af: 0x2061, InvisibleTimes: 0x2062, it: 0x2062,
  InvisibleComma: 0x2063, ic: 0x2063 };

const BLANK = /^[ \t]*$/;
/** Container markers that can precede a block start: blockquote `>` and list items. */
const CONTAINER = /^(?:[ \t]{0,3}(?:>[ \t]?|[-*+][ \t]+|\d{1,9}[.)][ \t]+))*/;
const HTML_START = /^[ \t]{0,3}<(?:[A-Za-z]|\/[A-Za-z]|!|\?)/;
const FENCE = /^[ \t]{0,3}(`{3,}|~{3,})/;
const REF_DEF = /^[ \t]{0,3}\[((?:[^\[\]\\]|\\.){1,999})\]:/;
const FOOTNOTE_DEF = /^[ \t]{0,3}\[\^([^\]\s]{1,200})\]:/;

type Ctx = "para" | "html" | "fence";
interface Line { start: number; end: number /* exclusive of the newline */; ctx: Ctx; fenceMark?: boolean }

/** Classifies each line: raw-HTML context, fenced code, or markdown paragraph text. Conservative toward "html". */
function classify(src: string): Line[] {
  const lines: Line[] = [];
  let pos = 0;
  let inHtml = false, fence: string | null = null;
  while (pos <= src.length) {
    const nl = src.indexOf("\n", pos);
    const end = nl < 0 ? src.length : nl;
    const text = src.slice(pos, end).replace(/\r$/, "");
    const stripped = text.replace(CONTAINER, "");
    let ctx: Ctx = "para";
    let fenceMark = false;
    if (fence) {
      ctx = "fence";
      const m = FENCE.exec(stripped);
      if (m && m[1][0] === fence[0] && m[1].length >= fence.length && BLANK.test(stripped.slice(m[0].length))) { fence = null; fenceMark = true; }
    } else if (inHtml) {
      if (BLANK.test(stripped)) inHtml = false; else ctx = "html";
    } else if (HTML_START.test(stripped) || HTML_START.test(text)) {
      inHtml = true; ctx = "html";
    } else {
      const m = FENCE.exec(stripped);
      // A backtick fence's info string may not contain a backtick.
      if (m && !(m[1][0] === "`" && stripped.slice(m[0].length).includes("`"))) { fence = m[1]; ctx = "fence"; fenceMark = true; }
    }
    lines.push({ start: pos, end, ctx, fenceMark });
    if (nl < 0) break;
    pos = nl + 1;
  }
  return lines;
}

interface DetailsFrame { collapsed: boolean; summary: "none" | "in" | "done"; content: boolean }

/** `visible` and `hidden` for one text. Never throws; on any internal error the whole text is dropped and counted. */
export function visibleOf(src: string): { visible: string; hidden: Hidden } {
  try { return scan(src); } catch {
    return { visible: "", hidden: { html_comments: 1, invisible_chars: 0, link_title_text: 0, collapsed: 0 } };
  }
}

function scan(src: string): { visible: string; hidden: Hidden } {
  const hidden: Hidden = { html_comments: 0, invisible_chars: 0, link_title_text: 0, collapsed: 0 };
  const lines = classify(src);
  const ctxAt = new Array<Ctx>(src.length + 1);
  for (const l of lines) for (let i = l.start; i <= Math.min(l.end, src.length); i++) ctxAt[i] = l.ctx;
  for (let i = 0; i <= src.length; i++) if (!ctxAt[i]) ctxAt[i] = "para";
  // Footnote labels referred to anywhere outside their own definition line.
  const refs = new Set<string>();
  for (const m of src.matchAll(/\[\^([^\]\s]{1,200})\](?!:)/g)) refs.add(m[1].toLowerCase());

  const out: string[] = [];
  const stack: DetailsFrame[] = [];
  const shown = (): boolean => stack.every((f) => !f.collapsed || f.summary === "in");
  const emit = (s: string): void => {
    if (!s) return;
    for (const f of stack) if (f.summary !== "in" && /\S/.test(s)) f.content = true;
    if (shown()) out.push(s);
  };
  const paraEnd = (from: number): number => { // the next blank line, or the end
    const m = /\n[ \t]*\r?\n/.exec(src.slice(from));
    return m ? from + m.index : src.length;
  };

  let i = 0;
  let lineStart = true;
  while (i < src.length) {
    const ctx = ctxAt[i];
    const c = src[i];
    // ---- Line starts: definitions (markdown context only) ----
    if (lineStart && ctx === "para") {
      lineStart = false;
      const rest = src.slice(i);
      const fn = FOOTNOTE_DEF.exec(rest);
      if (fn && !refs.has(fn[1].toLowerCase())) {
        const e = defEnd(src, i);
        hidden.html_comments++;
        i = e; continue;
      }
      const rd = !fn ? REF_DEF.exec(rest) : null;
      if (rd) {
        const d = parseRefDef(src, i + rd[0].length);
        if (d) {
          if (d.title) hidden.link_title_text++; else hidden.html_comments++;
          i = d.end; continue;
        }
      }
    }
    if (c === "\n") { emit(c); i++; lineStart = true; continue; }
    lineStart = false;
    // ---- Fenced code: literal, except raw hidden characters ----
    if (ctx === "fence") {
      const cp = src.codePointAt(i)!;
      const w = cp > 0xffff ? 2 : 1;
      if (isHiddenChar(cp)) hidden.invisible_chars++; else emit(src.slice(i, i + w));
      // `<details` opens still count in code, so a fake close elsewhere cannot end a real block early.
      if (c === "<" && /^<details(?=[\s>/])/i.test(src.slice(i, i + 9))) openDetails(src, i);
      i += w; continue;
    }
    // ---- Code spans (paragraph context only): literal ----
    if (c === "`" && ctx === "para" && !escaped(src, i)) {
      const run = /^`+/.exec(src.slice(i))![0].length;
      const close = findCodeClose(src, i + run, run, paraEnd(i));
      if (close >= 0) {
        for (let j = i; j < close + run;) {
          const cp = src.codePointAt(j)!;
          const w = cp > 0xffff ? 2 : 1;
          if (isHiddenChar(cp)) hidden.invisible_chars++;
          else if (src[j] === "<" && /^<details(?=[\s>/])/i.test(src.slice(j, j + 9))) { openDetails(src, j); emit(src[j]); }
          else emit(src.slice(j, j + w));
          j += w;
        }
        i = close + run; continue;
      }
      emit(src.slice(i, i + run)); i += run; continue;
    }
    // ---- Raw HTML: comments and their relatives ----
    if (c === "<") {
      const limit = ctx === "html" ? src.length : paraEnd(i);
      if (src.startsWith("<!--", i)) {
        let e: number;
        if (src.startsWith(">", i + 4)) e = i + 5;
        else if (src.startsWith("->", i + 4)) e = i + 6;
        else {
          const a = src.indexOf("-->", i + 4), b = src.indexOf("--!>", i + 4);
          const cand = [a < 0 ? Infinity : a + 3, b < 0 ? Infinity : b + 4];
          e = Math.min(...cand);
          if (!Number.isFinite(e)) e = ctx === "html" ? src.length : -1;
          else if (ctx !== "html" && e > limit) e = -1;
        }
        if (e >= 0) { hidden.html_comments++; i = e; continue; }
        emit(c); i++; continue;
      }
      const pi = src.startsWith("<?", i) ? closeOf(src, i + 2, "?>", limit, ctx) :
        src.startsWith("<![CDATA[", i) ? closeOf(src, i + 9, "]]>", limit, ctx) :
        /^<![A-Za-z]/.test(src.slice(i, i + 3)) ? closeOf(src, i + 2, ">", limit, ctx) : -2;
      if (pi >= 0) { hidden.html_comments++; i = pi; continue; }
      if (pi === -1) { emit(c); i++; continue; }
      // ---- Tags ----
      const tag = parseTag(src, i);
      if (tag) {
        const name = tag.name.toLowerCase();
        if (tag.attrs.some((a) => a.name === "title" && a.value.trim() !== "")) hidden.link_title_text++;
        const keep = tag.attrs.filter((a) => a.name === "href" || a.name === "src");
        const dropped = tag.attrs.filter((a) => a.name !== "href" && a.name !== "src" && a.name !== "title" && a.name !== "open" && a.value.trim() !== "");
        if (dropped.length) hidden.link_title_text++;
        const text = tag.close ? `</${name}>` : `<${name}${keep.map((a) => ` ${a.name}="${a.value}"`).join("")}${name === "details" && tag.attrs.some((a) => a.name === "open") ? " open" : ""}>`;
        if (name === "details" && !tag.close) {
          const collapsed = !tag.attrs.some((a) => a.name === "open");
          emit(text);
          if (collapsed) hidden.collapsed++;
          stack.push({ collapsed, summary: "none", content: false });
        } else if (name === "details" && tag.close) {
          emit(text);
          stack.pop();
        } else if (name === "summary" && !tag.close && stack.length && stack[stack.length - 1].summary === "none" && !stack[stack.length - 1].content) {
          stack[stack.length - 1].summary = "in";
          emit(text);
        } else if (name === "summary" && tag.close && stack.length && stack[stack.length - 1].summary === "in") {
          emit(text);
          stack[stack.length - 1].summary = "done";
        } else emit(text);
        i = tag.end; continue;
      }
      emit(c); i++; continue;
    }
    // ---- Character references ----
    if (c === "&") {
      const m = /^&(?:#([0-9]{1,7})|#[xX]([0-9a-fA-F]{1,6})|([A-Za-z][A-Za-z0-9]{1,31}));/.exec(src.slice(i, i + 40));
      if (m) {
        const cp = m[1] ? Number(m[1]) : m[2] ? parseInt(m[2], 16) : HIDDEN_NAMED[m[3]] ?? -1;
        if (cp >= 0 && isHiddenChar(cp)) { hidden.invisible_chars++; i += m[0].length; continue; }
        emit(m[0]); i += m[0].length; continue;
      }
    }
    // ---- Markdown inline link and image titles (paragraph context) ----
    if (c === "]" && ctx === "para" && src[i + 1] === "(" && !escaped(src, i)) {
      const l = parseInlineLink(src, i + 2);
      if (l) {
        emit("](");
        emitText(l.dest);
        emit(")");
        if (l.title) hidden.link_title_text++;
        i = l.end; continue;
      }
    }
    // ---- Plain characters ----
    const cp = src.codePointAt(i)!;
    const w = cp > 0xffff ? 2 : 1;
    if (isHiddenChar(cp)) hidden.invisible_chars++; else emit(src.slice(i, i + w));
    i += w;
  }
  return { visible: out.join(""), hidden };

  function emitText(s: string): void {
    for (const ch of s) { const cp = ch.codePointAt(0)!; if (isHiddenChar(cp)) hidden.invisible_chars++; else emit(ch); }
  }
  /** A `<details` inside code: counted as an open so the region it may start is never shown by a later close. */
  function openDetails(s: string, at: number): void {
    const t = parseTag(s, at);
    const open = t ? t.attrs.some((a) => a.name === "open") : false;
    if (!open) hidden.collapsed++;
    stack.push({ collapsed: !open, summary: "none", content: false });
  }
}

function escaped(s: string, i: number): boolean { let n = 0; while (i - n - 1 >= 0 && s[i - n - 1] === "\\") n++; return n % 2 === 1; }

/** Closing backtick run of exactly `n`, before `limit`. */
function findCodeClose(s: string, from: number, n: number, limit: number): number {
  let j = from;
  while (j < limit) {
    const k = s.indexOf("`", j);
    if (k < 0 || k >= limit) return -1;
    let r = 0; while (s[k + r] === "`") r++;
    if (r === n) return k;
    j = k + r;
  }
  return -1;
}

/** End index after `close`, or the end of the text in raw-HTML context, or -1 (literal) in a paragraph. */
function closeOf(s: string, from: number, close: string, limit: number, ctx: Ctx): number {
  const e = s.indexOf(close, from);
  if (e >= 0 && (ctx === "html" || e < limit)) return e + close.length;
  return ctx === "html" ? s.length : -1;
}

interface Tag { name: string; close: boolean; attrs: { name: string; value: string }[]; end: number }
/** An HTML open or close tag at `i` (CommonMark's raw HTML tag grammar, quotes honored), or null. */
export function parseTag(s: string, i: number): Tag | null {
  const m = /^<(\/?)([A-Za-z][A-Za-z0-9-]*)/.exec(s.slice(i, i + 64));
  if (!m) return null;
  let j = i + m[0].length;
  const attrs: { name: string; value: string }[] = [];
  if (m[1]) {
    while (j < s.length && /\s/.test(s[j])) j++;
    return s[j] === ">" ? { name: m[2], close: true, attrs, end: j + 1 } : null;
  }
  for (;;) {
    const ws = /^\s*/.exec(s.slice(j, j + 4096))![0].length;
    j += ws;
    if (s[j] === ">") return { name: m[2], close: false, attrs, end: j + 1 };
    if (s.startsWith("/>", j)) return { name: m[2], close: false, attrs, end: j + 2 };
    if (!ws) return null;
    const a = /^([A-Za-z_:][A-Za-z0-9_.:-]*)/.exec(s.slice(j, j + 256));
    if (!a) return null;
    j += a[0].length;
    let value = "";
    const eq = /^\s*=\s*/.exec(s.slice(j, j + 256));
    if (eq) {
      j += eq[0].length;
      const q = s[j];
      if (q === '"' || q === "'") {
        const e = s.indexOf(q, j + 1);
        if (e < 0) return null;
        value = s.slice(j + 1, e); j = e + 1;
      } else {
        const u = /^[^\s"'=<>`]+/.exec(s.slice(j, j + 4096));
        if (!u) return null;
        value = u[0]; j += u[0].length;
      }
    }
    attrs.push({ name: a[1].toLowerCase(), value });
  }
}

/** `dest`, an optional title, and `)` after `](`, as CommonMark's inline link grammar reads them. */
function parseInlineLink(s: string, i: number): { dest: string; title: string | null; end: number } | null {
  let j = skipWs(s, i);
  let dest = "";
  if (s[j] === "<") {
    const e = s.indexOf(">", j + 1);
    if (e < 0 || s.slice(j + 1, e).includes("\n")) return null;
    dest = s.slice(j, e + 1); j = e + 1;
  } else {
    let depth = 0; const st = j;
    while (j < s.length) {
      const ch = s[j];
      if (ch === "\\" && j + 1 < s.length) { j += 2; continue; }
      if (/\s/.test(ch)) break;
      if (ch === "(") depth++;
      if (ch === ")") { if (depth === 0) break; depth--; }
      j++;
    }
    dest = s.slice(st, j);
  }
  const ws = skipWs(s, j);
  let title: string | null = null;
  if (ws > j && (s[ws] === '"' || s[ws] === "'" || s[ws] === "(")) {
    const t = parseTitle(s, ws);
    if (!t) return null;
    title = t.title; j = t.end;
  } else j = ws;
  j = skipWs(s, j);
  if (s[j] !== ")") return null;
  return { dest, title, end: j + 1 };
}

function parseTitle(s: string, i: number): { title: string; end: number } | null {
  const open = s[i], close = open === "(" ? ")" : open;
  let j = i + 1;
  while (j < s.length) {
    if (s[j] === "\\") { j += 2; continue; }
    if (s[j] === close) return { title: s.slice(i + 1, j), end: j + 1 };
    if (open === "(" && s[j] === "(") return null;
    if (s[j] === "\n" && /^\n[ \t]*\r?\n/.test(s.slice(j))) return null;
    j++;
  }
  return null;
}

const skipWs = (s: string, i: number): number => { let n = 0; while (i < s.length && /[ \t\r\n]/.test(s[i])) { if (s[i] === "\n" && ++n > 1) break; i++; } return i; };

/** A reference definition's destination and optional title after `[label]:`, through the end of its line. */
function parseRefDef(s: string, i: number): { title: string | null; end: number } | null {
  let j = skipWs(s, i);
  if (j >= s.length) return null;
  if (s[j] === "<") { const e = s.indexOf(">", j); if (e < 0) return null; j = e + 1; }
  else { const st = j; while (j < s.length && !/\s/.test(s[j])) j++; if (j === st) return null; }
  const afterDest = j;
  const ws = skipWs(s, j);
  if (ws > j && (s[ws] === '"' || s[ws] === "'" || s[ws] === "(")) {
    const t = parseTitle(s, ws);
    if (t) {
      const e = lineEnd(s, t.end);
      if (BLANK.test(s.slice(t.end, e))) return { title: t.title, end: e };
    }
  }
  const e = lineEnd(s, afterDest);
  if (!BLANK.test(s.slice(afterDest, e))) return null;
  return { title: null, end: e };
}
const lineEnd = (s: string, i: number): number => { const e = s.indexOf("\n", i); return e < 0 ? s.length : e; };
/** A footnote definition runs through its first line and any indented continuation lines. */
function defEnd(s: string, i: number): number {
  let e = lineEnd(s, i);
  while (e < s.length) {
    const n = lineEnd(s, e + 1);
    if (/^(?: {2,}|\t)\S/.test(s.slice(e + 1, n))) e = n; else break;
  }
  return e;
}

/** `AuthorTrust` from what the fold knows about the author. */
export function authorTrust(a: { role: "read" | "write" | "admin" | null; owner: boolean; isBot: boolean; isAgent: boolean;
                                 firstContribution: boolean }): AuthorTrust {
  const can_push = !a.isBot && (a.role === "write" || a.role === "admin");
  const association: AuthorAssociation = a.owner ? "OWNER" : a.role === "write" || a.role === "admin" ? "COLLABORATOR"
    : a.firstContribution ? "FIRST_TIME_CONTRIBUTOR" : a.role === "read" ? "CONTRIBUTOR" : "NONE";
  return { association, can_push, is_bot: a.isBot, is_agent: a.isAgent, first_contribution: a.firstContribution,
           trusted: can_push && !a.isBot && !a.isAgent };
}

export function trustView(text: string, author: AuthorTrust): TrustView {
  const { visible, hidden } = visibleOf(text);
  return { visible, hidden, author };
}
