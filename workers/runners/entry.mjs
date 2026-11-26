// `graft-runners`' entry (workers/runners/deploy.mjs): the Rust Worker's handlers, which `worker-build` writes to its build folder,
// and `RunnerBox`, the Durable Object class that runs hosted runners. `worker-build` exports only Rust classes, so the
// JS class is exported here and wrangler bundles it.
//
// `RunnerBox` (`box.mjs`; decisions/DECISIONS.md, `r23-runnerbox`): one runner per object on the runner account's one
// `durable_object`-policy container application, started with the platform's `ctx.container` and the image of the
// Worker version that runs the object. Release M3 (`r23-m3`) deleted the Sandbox SDK classes that ran runners before it.
import { DurableObject } from "cloudflare:workers";
import * as box from "./box.mjs";

export { default } from "./build/index.js";

export class RunnerBox extends DurableObject {
  constructor(ctx, env) {
    super(ctx, env);
    ctx.blockConcurrencyWhile(() => box.wake(this));
  }
  async arm(_name, instance, jit) {
    return box.arm(this, instance, jit);
  }
  async alarm() {
    return box.alarm(this);
  }
  async destroy() {
    return box.destroy(this);
  }
  sleep(ms) {
    return new Promise((r) => setTimeout(r, ms));
  }
}
