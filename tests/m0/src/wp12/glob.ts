// `PathGlob` and `RefPattern` (design/DESIGN.md, Shape, leases): gitignore-style, `*` within one segment, `**` across
// segments, anchored at the root unless the glob starts with `**/`, no negation. Two globs overlap when some path
// matches both, found by walking both segment lists.

const segRe = new Map<string, RegExp>();
function segRegex(seg: string): RegExp {
  let r = segRe.get(seg);
  if (!r) {
    r = new RegExp("^" + seg.split("*").map((s) => s.replace(/[.+?^${}()|[\]\\]/g, "\\$&")).join("[^/]*") + "$");
    segRe.set(seg, r);
  }
  return r;
}
const split = (s: string): string[] => s.split("/").filter((x, i, a) => x !== "" || i === a.length - 1);

function matchSegs(g: string[], gi: number, p: string[], pi: number): boolean {
  if (gi === g.length) return pi === p.length;
  if (g[gi] === "**") {
    for (let k = pi; k <= p.length; k++) if (matchSegs(g, gi + 1, p, k)) return true;
    return false;
  }
  if (pi === p.length) return false;
  return segRegex(g[gi]).test(p[pi]) && matchSegs(g, gi + 1, p, pi + 1);
}

/** True when `path` (a file path or a full ref name) matches `glob`. A trailing `/` in the glob means "everything under". */
export function globMatch(glob: string, path: string): boolean {
  const g = glob.endsWith("/") ? [...split(glob.slice(0, -1)), "**"] : split(glob);
  return matchSegs(g, 0, split(path), 0);
}

/** Two single segments overlap when some string matches both. Literal against pattern is exact; pattern against
 *  pattern compares the literal prefix and suffix, which is exact for one `*` each and conservative beyond that. */
function segOverlap(a: string, b: string): boolean {
  const wa = a.includes("*"), wb = b.includes("*");
  if (!wa && !wb) return a === b;
  if (!wa) return segRegex(b).test(a);
  if (!wb) return segRegex(a).test(b);
  const pa = a.slice(0, a.indexOf("*")), pb = b.slice(0, b.indexOf("*"));
  const sa = a.slice(a.lastIndexOf("*") + 1), sb = b.slice(b.lastIndexOf("*") + 1);
  return (pa.startsWith(pb) || pb.startsWith(pa)) && (sa.endsWith(sb) || sb.endsWith(sa));
}

function overlapSegs(a: string[], i: number, b: string[], j: number): boolean {
  if (i === a.length && j === b.length) return true;
  if (i < a.length && a[i] === "**") return overlapSegs(a, i + 1, b, j) || (j < b.length && overlapSegs(a, i, b, j + 1));
  if (j < b.length && b[j] === "**") return overlapSegs(a, i, b, j + 1) || (i < a.length && overlapSegs(a, i + 1, b, j));
  if (i === a.length || j === b.length) return false;
  return segOverlap(a[i], b[j]) && overlapSegs(a, i + 1, b, j + 1);
}

export function globOverlap(a: string, b: string): boolean {
  const sa = a.endsWith("/") ? [...split(a.slice(0, -1)), "**"] : split(a);
  const sb = b.endsWith("/") ? [...split(b.slice(0, -1)), "**"] : split(b);
  return overlapSegs(sa, 0, sb, 0);
}

/** The literal prefix of a glob, up to its first wildcard segment: the index key for Exclusive globs. */
export function literalPrefix(glob: string): string {
  const out: string[] = [];
  for (const s of split(glob)) { if (s.includes("*")) break; out.push(s); }
  return out.join("/");
}
