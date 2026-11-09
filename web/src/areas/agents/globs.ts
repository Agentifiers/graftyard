// PathGlob from design/AGENT-LAYER.md, Shape, `PathGlob`: gitignore-style, `*` within one
// segment and `**` across segments, anchored at the root unless it starts with `**/`, and
// no negation. The UI validates with it before a write, and the mock decides with it.

export const MAX_GLOBS = 32;

export function globProblem(glob: string): string | undefined {
  if (!glob) return "A glob cannot be empty.";
  if (glob.startsWith("/")) return `${glob}: globs are anchored at the root; drop the leading /.`;
  if (glob.startsWith("!")) return `${glob}: negation is not supported.`;
  if (glob.length > 1024) return `${glob}: longer than 1024 characters.`;
  const segments = glob.split("/");
  if (segments.some((s) => s === "")) return `${glob}: has an empty path segment.`;
  if (segments.some((s) => s === "." || s === "..")) return `${glob}: . and .. are not allowed.`;
  if (segments.some((s) => s.includes("**") && s !== "**"))
    return `${glob}: ** must be a whole segment.`;
  return undefined;
}

// An Exclusive lease needs a literal first segment, so `**`, `*`, and `*/...` are refused.
export const literalFirstSegment = (glob: string) => !glob.split("/")[0].includes("*");

export function parseGlobs(text: string): string[] {
  return text
    .split(/[\n,]/)
    .map((s) => s.trim())
    .filter(Boolean);
}

function segmentRegex(segment: string) {
  const body = segment.replace(/[.+?^${}()|[\]\\]/g, "\\$&").replace(/\*/g, "[^/]*");
  return new RegExp(`^${body}$`);
}

function segmentsOverlap(a: string, b: string) {
  const aWild = a.includes("*");
  const bWild = b.includes("*");
  if (!aWild && !bWild) return a === b;
  if (!aWild) return segmentRegex(b).test(a);
  if (!bWild) return segmentRegex(a).test(b);
  // Two wildcard segments share a name when their literal ends agree.
  const [aHead, bHead] = [a.split("*")[0], b.split("*")[0]];
  const [aTail, bTail] = [a.split("*").at(-1)!, b.split("*").at(-1)!];
  return (
    (aHead.startsWith(bHead) || bHead.startsWith(aHead)) &&
    (aTail.endsWith(bTail) || bTail.endsWith(aTail))
  );
}

function walk(a: string[], b: string[]): boolean {
  if (a.length === 0 && b.length === 0) return true;
  if (a[0] === "**") return walk(a.slice(1), b) || (b.length > 0 && walk(a, b.slice(1)));
  if (b[0] === "**") return walk(a, b.slice(1)) || (a.length > 0 && walk(a.slice(1), b));
  if (a.length === 0 || b.length === 0) return false;
  return segmentsOverlap(a[0], b[0]) && walk(a.slice(1), b.slice(1));
}

// Two globs overlap when some path matches both.
export const globsOverlap = (a: string, b: string) => walk(a.split("/"), b.split("/"));

export const anyOverlap = (a: readonly string[], b: readonly string[]) =>
  a.some((x) => b.some((y) => globsOverlap(x, y)));
