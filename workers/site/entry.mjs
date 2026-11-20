// The site script's entry (workers/site/deploy.mjs): the Rust Worker's handlers and classes, which `worker-build`
// writes to its build folder, and the SSH front's classes (`ssh.mjs`). `worker-build` exports only Rust classes, so the
// JS classes are exported here and wrangler bundles them, as `graft-runners` does (workers/runners/entry.mjs).
import { DurableObject, WorkerEntrypoint } from "cloudflare:workers";
import * as ssh from "./ssh.mjs";

export * from "./build/index.js";
export { default } from "./build/index.js";

export class SshFront extends DurableObject {
  async connect(socket) {
    return ssh.connect(this, socket);
  }
  async alarm() {
    return ssh.alarm(this);
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
