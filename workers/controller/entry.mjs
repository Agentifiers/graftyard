// `graft-controller`'s entry (`deploy.mjs`): the Rust Worker's handlers and its `TaskBox` Durable Object class, which
// `worker-build` writes to its build folder, and `Outbound`, the Sandbox's outbound router entrypoint. `worker-build`
// exports no Rust `WorkerEntrypoint` with props, so `Outbound` is this small JS class, as `graft-site`'s `SshOutbound`
// is. Its props name the task and the Sandbox (`src/taskbox.rs`, `intercept`), and it hands each intercepted request
// to that task's `TaskBox` with the props in `x-graftyard-outbound`. A Durable Object is reached only through this
// script's own code, never from the Internet, so no outside caller can set that header on a box's request
// [doc: https://developers.cloudflare.com/durable-objects/best-practices/architecture-guidance/]. The box decides and
// forwards (`graftyard_git::outbound`); no token or key passes through this file.
import { WorkerEntrypoint } from "cloudflare:workers";

export * from "./build/index.js";
export { default } from "./build/index.js";

export class Outbound extends WorkerEntrypoint {
  async fetch(request) {
    const { task, sandbox } = this.ctx.props;
    const r = new Request(request, { redirect: "manual" });
    r.headers.set("x-graftyard-outbound", JSON.stringify({ task, sandbox }));
    return this.env.TASKBOX.getByName(task).fetch(r);
  }
}
