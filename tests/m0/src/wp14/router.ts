// M0-WP14 (L0-02): the controller's outbound router, a prototype of `outbound` and its `Allowed` arms
// (design/AGENT-LAYER.md, Shape, "Controller"). It runs as a `WorkerEntrypoint` outside the sandbox. The controller
// registers it with `container.interceptOutboundHttps("*")` and `container.interceptAllOutboundHttp()` on a container
// started with `enableInternet: false`, so every HTTP (port 80) and HTTPS (port 443) request the sandbox makes arrives
// here, and every other port and protocol has no route out
// (https://developers.cloudflare.com/containers/api/durable-object-container/#interceptoutboundhttps).
//
// Default deny: a request that matches no arm is refused. The arms:
// - `GraftyardHost`: smart HTTP on the base and the push target, REST under `/api/v3/repos/{base or push target}/`.
//   The router always removes `Authorization`, `Cookie`, `Host`, and URL userinfo, then attaches the task's token: the
//   `graftyard-agent` token for the base and the `graftyard-workspace` token for the push target (design/AGENT-LAYER.md,
//   A3 step 5). It refuses receive-pack on the base, any receive-pack command outside the task's prefix, and every
//   status or check-run write. GraphQL, gists, uploads, and every other owner path are refused.
// - `ModelInference`: the model host's one inference path, POST only, with the JSON body parsed. `mcp_servers`,
//   server-side tool types, and file and batch endpoints are refused. The model key is attached here.
// - `Registry`: the registry host, GET only. Unknown Go module paths are refused.
// The router never follows a redirect (`redirect: "manual"`), so a 3xx from an allowed host goes back to the sandbox,
// and the sandbox's next request goes through the router again.
//
// Each response carries `x-wp14-router: 1` and either `x-wp14-allow: <arm>` or `x-wp14-refused: <rule>`. The harness
// uses these headers to count each attempt and to classify how it was blocked.

import { WorkerEntrypoint } from "cloudflare:workers";

/** The task's identity and allowlist, passed as props when the controller registers the intercept. */
export interface RouterProps {
  hostName: string;            // the Graftyard host (stand-in `graft-wp14-host`)
  modelHost: string;           // the model inference host (stand-in `graft-wp14-model`)
  registryHost: string;        // the package registry host (stand-in `graft-wp14-registry`)
  base: string;                // "owner/repo", read with the graftyard-agent token
  pushTarget: string;          // "owner/repo", the workspace fork in fork mode
  prefix: string;              // the task's ref prefix, e.g. "refs/heads/graftyard/<task>/"
  modelPath: string;           // the one allowed inference path
  goKnownModules: string[];    // Go module paths the registry mirror holds; others are refused
}

export interface RouterEnv {
  AGENT_TOKEN: string;         // graftyard-agent installation token (contents: read on the base). Never in the sandbox.
  WORKSPACE_TOKEN: string;     // graftyard-workspace installation token (the push target). Never in the sandbox.
  MODEL_KEY: string;           // the model key. Never in the sandbox.
}

export interface Refused { rule: string; text: string; status: number }

const SERVER_TOOL_TYPES = ["web_search", "web_fetch", "code_execution", "code_interpreter", "computer_use", "bash",
  "text_editor", "file_search", "retrieval", "mcp", "image_generation"];
const FILE_BATCH = ["/files", "/batches", "/uploads", "/fine_tuning", "/assistants", "/vector_stores", "/messages/batches"];

export class OutboundRouter extends WorkerEntrypoint<RouterEnv, RouterProps> {
  async fetch(request: Request): Promise<Response> {
    const r = await route(request, this.ctx.props, this.env);
    if (!(r instanceof Request)) {
      return new Response(`refused: ${r.text}\n`, { status: r.status, headers: { "x-wp14-refused": r.rule, "x-wp14-router": "1" } });
    }
    const arm = r.headers.get("x-wp14-arm") ?? "unknown";
    r.headers.delete("x-wp14-arm");
    const upstream = await fetch(r);
    const h = new Headers(upstream.headers);
    h.set("x-wp14-allow", arm);
    h.set("x-wp14-router", "1");
    return new Response(upstream.body, { status: upstream.status, headers: h });
  }
}

/** The routing decision. Returns the forwardable request with its credential attached, or a refusal. */
export async function route(request: Request, p: RouterProps, env: RouterEnv): Promise<Request | Refused> {
  const url = new URL(request.url);
  // The router adds credentials to HTTPS only. A credential on plain HTTP would cross the Internet unencrypted
  // (https://developers.cloudflare.com/sandbox/concepts/security/), so plain HTTP is refused on every arm.
  if (url.protocol !== "https:") return { rule: "PlainHttp", text: "plain HTTP is refused", status: 403 };
  if (url.port !== "" && url.port !== "443") return { rule: "Port", text: `port ${url.port} is refused`, status: 403 };
  if (request.method === "CONNECT") return { rule: "Connect", text: "CONNECT is refused", status: 403 };

  const host = url.hostname.toLowerCase().replace(/\.$/, "");
  const path = url.pathname;

  // No arm needs a percent-encoded or dot-segment path, and either can make the router and the upstream disagree about
  // which resource is named, so both are refused before any arm matches.
  if (/%|\/\.\.?(\/|$)|\/\//.test(path)) return { rule: "PathEncoding", text: "encoded, dot-segment, or empty-segment paths are refused", status: 403 };

  if (host === p.registryHost) {
    if (request.method !== "GET" || url.search !== "") return { rule: "RegistryMethod", text: "the registry takes a plain GET only", status: 403 };
    const m = /^\/go\/(.+?)\/(@latest|@v\/list|@v\/v[0-9][^/]*\.(info|mod|zip))$/.exec(path);
    if (!m) return { rule: "RegistryPath", text: `${path} is not a module path this mirror serves`, status: 403 };
    const mod = goUnescape(m[1]);
    if (mod === null || !p.goKnownModules.includes(mod)) return { rule: "UnknownGoModule", text: `Go module ${m[1]} is not in the mirror`, status: 403 };
    return armed(sanitize(request, url, null), "Registry");
  }

  if (host === p.modelHost) {
    if (FILE_BATCH.some((s) => path.startsWith(s) || path.startsWith("/v1" + s))) {
      return { rule: "ModelFileBatch", text: `file and batch endpoints are refused (${path})`, status: 403 };
    }
    if (path !== p.modelPath) return { rule: "ModelPath", text: `only ${p.modelPath} is allowed on the model host`, status: 403 };
    if (request.method !== "POST") return { rule: "ModelMethod", text: "the inference path takes POST only", status: 403 };
    const raw = await bodyBytes(request);
    let body: unknown;
    try { body = JSON.parse(new TextDecoder().decode(raw)); } catch { return { rule: "ModelBody", text: "the body must be JSON", status: 400 }; }
    const bad = modelBodyRefusal(body);
    if (bad) return { rule: "ModelPayload", text: bad, status: 403 };
    // Forward the body the router checked, re-serialized, never the sandbox's bytes. A duplicate key or an encoding the
    // upstream parses differently could otherwise carry a field past the check.
    const fwd = sanitize(request, url, new TextEncoder().encode(JSON.stringify(body)));
    for (const k of ["content-length", "content-encoding", "transfer-encoding"]) fwd.headers.delete(k);
    fwd.headers.set("content-type", "application/json");
    fwd.headers.set("authorization", `Bearer ${env.MODEL_KEY}`);
    return armed(fwd, "ModelInference");
  }

  if (host !== p.hostName) return { rule: "UnlistedHost", text: `${host} is on no Allowed arm`, status: 403 };

  // GraftyardHost.
  const git = /^\/([^/]+)\/([^/]+?)\.git(\/.*)?$/.exec(path);
  const rest = /^\/api\/v3\/repos\/([^/]+)\/([^/]+)(\/.*)?$/.exec(path);
  if (!git && !rest) {
    const lp = path.toLowerCase();
    if (/^\/(api\/)?graphql/.test(lp)) return { rule: "GraphQL", text: "GraphQL is refused", status: 403 };
    if (/^\/(api\/v3\/)?gists/.test(lp)) return { rule: "Gist", text: "gists are refused", status: 403 };
    if (/^\/(api\/)?uploads/.test(lp)) return { rule: "Upload", text: "uploads are refused", status: 403 };
    return { rule: "OwnerPath", text: `${path} is not git or REST on the task's repositories`, status: 403 };
  }
  const m = (git ?? rest)!;
  const repo = `${m[1]}/${m[2]}`.toLowerCase();
  const isBase = repo === p.base, isTarget = repo === p.pushTarget;
  if (!isBase && !isTarget) return { rule: "OtherRepo", text: `${repo} is not the task's base or push target`, status: 403 };
  const token = isTarget ? env.WORKSPACE_TOKEN : env.AGENT_TOKEN;

  if (git) {
    const tail = git[3] ?? "";
    const receive = tail === "/git-receive-pack" || (tail === "/info/refs" && url.searchParams.get("service") === "git-receive-pack");
    if (receive) {
      if (!isTarget) return { rule: "ReceivePackBase", text: `receive-pack is refused on ${repo}; the task pushes to ${p.pushTarget}`, status: 403 };
      if (tail === "/git-receive-pack") {
        const raw = await bodyBytes(request);
        const refs = await receiveCommands(raw, request.headers.get("content-encoding"));
        if (refs === null) return { rule: "ReceivePackParse", text: "the receive-pack request could not be parsed", status: 400 };
        const out = refs.find((r) => !r.startsWith(p.prefix));
        if (out !== undefined || refs.length === 0) return { rule: "OutsidePrefix", text: `refused: ${out ?? "(no ref)"} is outside ${p.prefix}`, status: 403 };
        const fwd = sanitize(request, url, raw);
        fwd.headers.set("authorization", basic(token));
        return armed(fwd, "GraftyardHost");
      }
    } else if (!(tail === "/info/refs" || tail === "/git-upload-pack" || tail === "/HEAD" || tail.startsWith("/objects/"))) {
      return { rule: "GitPath", text: `git path ${tail} is refused`, status: 403 };
    }
    const fwd = sanitize(request, url, request.method === "GET" || request.method === "HEAD" ? null : await bodyBytes(request));
    fwd.headers.set("authorization", basic(token));
    return armed(fwd, "GraftyardHost");
  }

  const sub = (rest![3] ?? "").toLowerCase();
  const write = request.method !== "GET" && request.method !== "HEAD";
  const override = request.headers.get("x-http-method-override");
  if ((write || override) && (/^\/statuses(\/|$)/.test(sub) || /^\/check-(runs|suites)(\/|$)/.test(sub) || /^\/commits\/[^/]+\/(statuses|check-runs|check-suites)/.test(sub))) {
    return { rule: "StatusOrCheckRun", text: `status and check-run writes are refused (${sub})`, status: 403 };
  }
  const fwd = sanitize(request, url, write ? await bodyBytes(request) : null);
  fwd.headers.set("authorization", `Bearer ${token}`);
  return armed(fwd, "GraftyardHost");
}

/** The model body refusal, or null. `mcp_servers` anywhere at the top level, and any server-side tool type. */
export function modelBodyRefusal(body: unknown): string | null {
  if (typeof body !== "object" || body === null || Array.isArray(body)) return "the body must be a JSON object";
  const b = body as Record<string, unknown>;
  if ("mcp_servers" in b) return "mcp_servers is refused";
  for (const key of ["tools", "builtin_tools"]) {
    const tools = b[key];
    if (tools === undefined) continue;
    if (!Array.isArray(tools)) return `${key} must be an array`;
    for (const t of tools) {
      const ty = typeof t === "object" && t !== null ? String((t as Record<string, unknown>).type ?? "") : "";
      // A client tool is `{type: "function", ...}` or has no type (Anthropic's custom tools). A typed server tool
      // carries a versioned type such as `web_search_20250305`, so match by prefix.
      if (ty && ty !== "function" && ty !== "custom") {
        const base = ty.toLowerCase();
        if (SERVER_TOOL_TYPES.some((s) => base === s || base.startsWith(s + "_") || base.startsWith(s + "-"))) return `server-side tool ${ty} is refused`;
        return `tool type ${ty} is not a client tool`;
      }
    }
  }
  return null;
}

/** The ref names of a receive-pack request's command list: pkt-lines up to the first flush. */
export async function receiveCommands(raw: Uint8Array, encoding: string | null): Promise<string[] | null> {
  let buf = raw;
  if (encoding && /gzip/i.test(encoding)) {
    buf = new Uint8Array(await new Response(new Blob([raw]).stream().pipeThrough(new DecompressionStream("gzip"))).arrayBuffer());
  }
  const dec = new TextDecoder();
  const refs: string[] = [];
  let off = 0;
  while (off + 4 <= buf.length) {
    const len = parseInt(dec.decode(buf.subarray(off, off + 4)), 16);
    if (Number.isNaN(len)) return null;
    if (len === 0) return refs;
    if (len < 4 || off + len > buf.length) return null;
    const line = dec.decode(buf.subarray(off + 4, off + len)).split("\0")[0].replace(/\n$/, "");
    const parts = line.split(" ");
    if (line.startsWith("shallow ")) { off += len; continue; }
    if (parts.length < 3) return null;
    refs.push(parts[2]);
    off += len;
  }
  return null;
}

function sanitize(request: Request, url: URL, body: Uint8Array | null): Request {
  const clean = new URL(url.toString());
  clean.username = "";
  clean.password = "";
  const h = new Headers(request.headers);
  for (const k of ["authorization", "cookie", "host", "proxy-authorization", "x-forwarded-host", "forwarded"]) h.delete(k);
  return new Request(clean.toString(), { method: request.method, headers: h, body, redirect: "manual" });
}

const armed = (r: Request, arm: string): Request => { r.headers.set("x-wp14-arm", arm); return r; };
const basic = (token: string): string => `Basic ${btoa(`x-access-token:${token}`)}`;
const bodyBytes = async (r: Request): Promise<Uint8Array> => new Uint8Array(await r.arrayBuffer());

/** Go's module proxy case-encoding: `!x` stands for `X`, and a raw upper-case letter is invalid
 *  (https://go.dev/ref/mod#goproxy-protocol). Returns the module path, or null if the escaping is invalid. */
export function goUnescape(s: string): string | null {
  let out = "";
  for (let i = 0; i < s.length; i++) {
    const ch = s[i];
    if (ch >= "A" && ch <= "Z") return null;
    if (ch === "!") {
      const n = s[i + 1];
      if (!n || n < "a" || n > "z") return null;
      out += n.toUpperCase();
      i++;
    } else out += ch;
  }
  return out;
}
