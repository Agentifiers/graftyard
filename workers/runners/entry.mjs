// `graft-runners`' entry (workers/runners/deploy.mjs): the Rust Worker's handlers, which `worker-build` writes to its build folder,
// and the Durable Object classes that run hosted runners. `worker-build` exports only Rust classes, so the JS classes
// are exported here and wrangler bundles them.
//
// - `RunnerBox` (`box.mjs`; decisions/DECISIONS.md, `r23-runnerbox`): one runner per object on the runner account's one
//   `durable_object`-policy container application, started with the platform's `ctx.container` and the image of the
//   Worker version that runs the object. Starts use it while the version's `RUNNER_BOX` is "1".
// - `SandboxStd3` and `SandboxStd4` (`launch.mjs`): the Sandbox SDK's class, one per instance type on two
//   `default`-policy applications, which serve runners started before `RUNNER_BOX` and are deleted at release M3.
import { DurableObject } from "cloudflare:workers";
import { Sandbox } from "@cloudflare/sandbox";
import * as box from "./box.mjs";
import * as launch from "./launch.mjs";

export { default } from "./build/index.js";
// The SDK asks the entry to export this too (its type declarations, `ContainerProxy`).
export { ContainerProxy } from "@cloudflare/sandbox";

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

// A runner's Sandbox starts its own process from its alarm (`launch.mjs`).
class RunnerSandbox extends Sandbox {
  async arm(name, cmd, opts) {
    await this.setSandboxName(name);
    await this.setKeepAlive(true);
    return launch.arm(this, cmd, opts);
  }
  // Run by the base class's `alarm()` for a due `schedule` row, which it deletes even when the callback throws, so
  // `launch` never throws.
  async launchRunner() {
    return launch.launch(this);
  }
  async destroy() {
    await launch.close(this);
    return super.destroy();
  }
}
export class SandboxStd3 extends RunnerSandbox {}
export class SandboxStd4 extends RunnerSandbox {}
