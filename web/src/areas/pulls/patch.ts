// Pure helpers over a GitHub `patch` (hunks only, no file headers). The diff views and
// the mock both use them, so a comment's anchor means the same line on both sides.

export type Side = "LEFT" | "RIGHT";

export type CodeLine =
  | { kind: "context"; text: string; old: number; new: number }
  | { kind: "add"; text: string; new: number; old?: undefined }
  | { kind: "del"; text: string; old: number; new?: undefined };

export type Line = { kind: "hunk"; text: string } | CodeLine;

const HUNK = /^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@/;

export function parsePatch(patch: string): Line[] {
  const lines: Line[] = [];
  let oldNo = 0;
  let newNo = 0;
  for (const raw of patch.split("\n")) {
    const hunk = HUNK.exec(raw);
    if (hunk) {
      oldNo = Number(hunk[1]);
      newNo = Number(hunk[2]);
      lines.push({ kind: "hunk", text: raw });
    } else if (raw.startsWith("+")) {
      lines.push({ kind: "add", text: raw.slice(1), new: newNo++ });
    } else if (raw.startsWith("-")) {
      lines.push({ kind: "del", text: raw.slice(1), old: oldNo++ });
    } else if (!raw.startsWith("\\")) {
      lines.push({ kind: "context", text: raw.slice(1), old: oldNo++, new: newNo++ });
    }
  }
  return lines;
}

// Where a review comment attaches: a deleted line on the old side, anything else on the new.
export type Anchor = { side: Side; line: number };

export function anchorOf(l: CodeLine): Anchor {
  return l.kind === "del" ? { side: "LEFT", line: l.old } : { side: "RIGHT", line: l.new };
}

export function sameAnchor(a: Anchor, b: Anchor) {
  return a.side === b.side && a.line === b.line;
}

export function lineAt(lines: Line[], a: Anchor): CodeLine | undefined {
  return lines.find(
    (l): l is CodeLine =>
      l.kind !== "hunk" &&
      (a.side === "LEFT"
        ? l.kind !== "add" && l.old === a.line
        : l.kind !== "del" && l.new === a.line),
  );
}

// Split view: a run of deletions sits beside the run of additions that follows it.
export type SplitRow =
  | { kind: "hunk"; text: string }
  | { kind: "pair"; left?: CodeLine; right?: CodeLine };

export function splitLines(lines: Line[]): SplitRow[] {
  const rows: SplitRow[] = [];
  let dels: CodeLine[] = [];
  let adds: CodeLine[] = [];
  const flush = () => {
    for (let i = 0; i < Math.max(dels.length, adds.length); i++) {
      rows.push({ kind: "pair", left: dels[i], right: adds[i] });
    }
    dels = [];
    adds = [];
  };
  for (const l of lines) {
    if (l.kind === "del") {
      if (adds.length) flush();
      dels.push(l);
    } else if (l.kind === "add") {
      adds.push(l);
    } else {
      flush();
      rows.push(l.kind === "hunk" ? l : { kind: "pair", left: l, right: l });
    }
  }
  flush();
  return rows;
}

// GitHub's diff_hunk for a comment: the hunk header through the commented line.
export function hunkUpTo(patch: string, a: Anchor): string | undefined {
  const lines = parsePatch(patch);
  const raw = patch.split("\n").filter((r) => !r.startsWith("\\"));
  let start = 0;
  for (let i = 0; i < lines.length; i++) {
    const l = lines[i];
    if (l.kind === "hunk") start = i;
    else if (
      sameAnchor(anchorOf(l), a) ||
      (l.kind === "context" && a.side === "LEFT" && l.old === a.line)
    ) {
      return raw.slice(start, i + 1).join("\n");
    }
  }
  return undefined;
}
