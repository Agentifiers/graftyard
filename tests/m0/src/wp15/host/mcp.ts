// M0-WP13: the `graft-mcp` tool deny-list that `call` and Code Mode's `codemode.request()` share (design/AGENT-LAYER.md,
// Shape, "graft-mcp": `WriteScope`, `CodeMode`). The host also enforces `WriteScope` from the scoped token's `Tokens`
// row, so this is the first of two layers; L0-15 measures both. Pure.

/** Why a request is refused before it reaches the edge, or null. `write` is the token's `WriteScope` from its row. */
export function codeModeDenied(method: string, path: string, body: unknown, write: string): string | null {
  const p = path.split("?")[0];
  // Every tool is one to three REST calls through the edge: no GraphQL, no harness or other paths.
  if (!/^\/api\/(v3\/repos|graftyard\/v1\/repos)\/[^/]+\/[^/]+(\/|$)/.test(p)) return `path ${p} is not a repository REST path`;
  if (/\.\.|\/\//.test(p)) return "path is not normalized";
  if (method === "GET") return null;
  if (write === "Read") return "this grant is read-only";
  if (write === "WriteApproveMerge") return null;
  // Write without approve and merge: the tools that approve or merge are hidden, and their REST shapes refused.
  const rest = p.replace(/^\/api\/(v3|graftyard\/v1)\/repos\/[^/]+\/[^/]+/, "");
  const ev = typeof body === "object" && body ? String((body as Record<string, unknown>).event ?? "").toUpperCase() : "";
  if (/^\/pulls\/\d+\/reviews(\/\d+\/events)?$/.test(rest) && ev === "APPROVE") return "approve needs the WriteApproveMerge grant";
  if (/^\/pulls\/\d+\/merge$/.test(rest)) return "merge needs the WriteApproveMerge grant";
  if (rest === "/merges") return "merge needs the WriteApproveMerge grant";
  if (/^\/pulls\/\d+\/auto-merge$/.test(rest)) return "merge needs the WriteApproveMerge grant";
  return null;
}
