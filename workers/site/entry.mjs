// The site script's entry (workers/site/deploy.mjs): the Rust Worker's handlers and classes, which `worker-build`
// writes to its build folder, and the SSH front's classes (`ssh.mjs`). `worker-build` exports only Rust classes, so the
// JS classes are exported here and wrangler bundles them, as `graft-runners` does (workers/runners/entry.mjs).
// `SshFront` takes sessions over `connect` and answers `GET /status` (`ssh.status`) on `fetch`, which the site's
// Registry serves at `/ssh_front` for graft-ops' `SshHostKeys` probe, and, on a test install's `SSH_KILL` version
// only, `POST /kill` (`ssh.kill`), which a test-faults Registry serves at `/ssh_front_kill`.
import { DurableObject, WorkerEntrypoint } from "cloudflare:workers";
import * as ssh from "./ssh.mjs";

export * from "./build/index.js";
export { default } from "./build/index.js";

export class SshFront extends DurableObject {
  async connect(socket) {
    return ssh.connect(this, socket);
  }
  async fetch(request) {
    const url = new URL(request.url);
    if (request.method === "GET" && url.pathname === "/status") return Response.json(await ssh.status(this));
    if (request.method === "POST" && url.pathname === "/kill") return ssh.kill(this);
    return new Response("Not Found", { status: 404 });
  }
  sleep(ms) {
    return new Promise((r) => setTimeout(r, ms));
  }
}

export class SshOutbound extends WorkerEntrypoint {
  async fetch(request) {
    return ssh.outbound(this.ctx.props, this.env, request);
  }
}
