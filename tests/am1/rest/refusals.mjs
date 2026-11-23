// The refusal-shape tally (FRAME, "Operator decision, 2026-11-23, B0-33's merge-queue and refusal-shape lines"): every
// answered non-2xx, counted by the edge's route kind (`route_kind` in `workers/edge/src/lib.rs`, so a row joins the
// edge's `fivexx` lines), caller class, read or write, status, `Retry-After` value, and reason (a JSON body's `message`,
// else the body's first line, with hex ids as H and numbers as N, as `tests/fanout/fivexxwhy.py` reads them). a1.mjs
// writes `rows()` as its row's `refusals`; `tests/fanout/refusalshape.py` reads them.
const AGENTS = "/api/graftyard/v1";

/** The edge's `route_kind` for `method` and `path` (no query). */
export function routeKind(method, path) {
  const p = path.split("?")[0];
  const agents = p.startsWith(AGENTS);
  if (p.endsWith("/git-upload-pack")) return "upload-pack";
  if (p.endsWith("/git-receive-pack")) return "receive-pack";
  if (p.endsWith("/info/refs")) return "info-refs";
  if (p.endsWith("/graphql")) return "graphql";
  if (agents && p.includes("/renew")) return "renew";
  if (agents && p.includes("/leases")) return "lease";
  if (agents && p.includes("/tasks")) return "task";
  if (agents) return "agents";
  if (p.includes("/merge")) return "merge";
  if (p.includes("/repos/") && method === "GET") return "rest-repo-read";
  if (p.includes("/repos/")) return "rest-repo-write";
  return "other";
}

/** Whether the request writes: a receive-pack POST, a GraphQL mutation, or any other non-GET API call. */
export function isWrite(method, path, query) {
  const kind = routeKind(method, path);
  if (kind === "receive-pack") return true;
  if (kind === "upload-pack" || kind === "info-refs") return false;
  if (kind === "graphql") return /^\s*mutation\b/.test(query ?? "");
  return method !== "GET" && method !== "HEAD";
}

const reason = (body) => {
  const msg = body && typeof body === "object" ? body.message : null;
  const s = typeof msg === "string" ? msg : typeof body === "string" ? body : "";
  return s.split("\n")[0].trim().replace(/[0-9a-f]{16,}/g, "H").replace(/\d+/g, "N").slice(0, 120);
};

export class Refusals {
  #m = new Map();
  /** One answer. `headers` is a fetch `Headers` or a plain object with lower-case keys; `body` a parsed JSON value or
   *  text. 2xx answers and no answer (status 0) are not counted. */
  note({ method, path, cls, write, status, headers, body }) {
    if (!status || (status >= 200 && status < 300)) return;
    const ra = (typeof headers?.get === "function" ? headers.get("retry-after") : headers?.["retry-after"]) ?? null;
    const row = { route: routeKind(method, path), cls, write: !!write, status, retryAfter: ra, why: reason(body) };
    const k = JSON.stringify(row);
    this.#m.set(k, (this.#m.get(k) ?? 0) + 1);
  }
  rows() {
    return [...this.#m].map(([k, n]) => ({ ...JSON.parse(k), n })).sort((a, b) => b.n - a.n);
  }
}
