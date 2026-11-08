// Line diffs, shared by the mock (commit files, blame) and the editor's change preview.
// Pure functions over arrays of lines; a text's trailing newline is not a line.

export function splitLines(text: string): string[] {
  if (text === "") return [];
  const lines = text.split("\n");
  if (lines.at(-1) === "") lines.pop();
  return lines;
}

// Index pairs [i, j] with a[i] === b[j], forming one longest common subsequence.
export function matchLines(a: readonly string[], b: readonly string[]): [number, number][] {
  let head = 0;
  while (head < a.length && head < b.length && a[head] === b[head]) head++;
  let endA = a.length;
  let endB = b.length;
  while (endA > head && endB > head && a[endA - 1] === b[endB - 1]) {
    endA--;
    endB--;
  }
  const n = endA - head;
  const m = endB - head;
  const w = m + 1;
  const lcs = new Uint32Array((n + 1) * w);
  for (let i = n - 1; i >= 0; i--) {
    for (let j = m - 1; j >= 0; j--) {
      lcs[i * w + j] =
        a[head + i] === b[head + j]
          ? lcs[(i + 1) * w + j + 1] + 1
          : Math.max(lcs[(i + 1) * w + j], lcs[i * w + j + 1]);
    }
  }
  const pairs: [number, number][] = [];
  for (let k = 0; k < head; k++) pairs.push([k, k]);
  let i = 0;
  let j = 0;
  while (i < n && j < m) {
    if (a[head + i] === b[head + j]) {
      pairs.push([head + i, head + j]);
      i++;
      j++;
    } else if (lcs[(i + 1) * w + j] >= lcs[i * w + j + 1]) i++;
    else j++;
  }
  for (let k = 0; endA + k < a.length; k++) pairs.push([endA + k, endB + k]);
  return pairs;
}

type Op = { sign: " " | "-" | "+"; text: string; oldNo: number; newNo: number };

function operations(a: readonly string[], b: readonly string[]): Op[] {
  const ops: Op[] = [];
  let i = 0;
  let j = 0;
  for (const [pi, pj] of matchLines(a, b)) {
    while (i < pi) ops.push({ sign: "-", text: a[i], oldNo: i++, newNo: j });
    while (j < pj) ops.push({ sign: "+", text: b[j], oldNo: i, newNo: j++ });
    ops.push({ sign: " ", text: a[i], oldNo: i++, newNo: j++ });
  }
  while (i < a.length) ops.push({ sign: "-", text: a[i], oldNo: i++, newNo: j });
  while (j < b.length) ops.push({ sign: "+", text: b[j], oldNo: i, newNo: j++ });
  return ops;
}

export type TextPatch = { patch: string; additions: number; deletions: number };

// A GitHub `patch`: hunks only, no file headers, three lines of context.
export function unifiedDiff(before: string, after: string, context = 3): TextPatch {
  const ops = operations(splitLines(before), splitLines(after));
  const changed = ops.flatMap((op, k) => (op.sign === " " ? [] : [k]));
  const hunks: string[] = [];
  let k = 0;
  while (k < changed.length) {
    let last = k;
    while (last + 1 < changed.length && changed[last + 1] - changed[last] <= context * 2 + 1) {
      last++;
    }
    const from = Math.max(0, changed[k] - context);
    const to = Math.min(ops.length, changed[last] + context + 1);
    const slice = ops.slice(from, to);
    const oldCount = slice.filter((o) => o.sign !== "+").length;
    const newCount = slice.filter((o) => o.sign !== "-").length;
    const oldStart = oldCount ? slice[0].oldNo + 1 : slice[0].oldNo;
    const newStart = newCount ? slice[0].newNo + 1 : slice[0].newNo;
    hunks.push(
      `@@ -${oldStart},${oldCount} +${newStart},${newCount} @@`,
      ...slice.map((o) => `${o.sign}${o.text}`),
    );
    k = last + 1;
  }
  return {
    patch: hunks.join("\n"),
    additions: ops.filter((o) => o.sign === "+").length,
    deletions: ops.filter((o) => o.sign === "-").length,
  };
}
