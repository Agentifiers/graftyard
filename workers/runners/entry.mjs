// `graft-runners`' entry (workers/runners/deploy.mjs): the Rust Worker's handlers, which `worker-build` writes to its build folder,
// and the Sandbox SDK's Durable Object class, once per instance type, because a container class has one instance type.
// `worker-build` exports only Rust classes, so the SDK's JS class is exported here and wrangler bundles both.
import { Sandbox } from "@cloudflare/sandbox";
import * as launch from "./launch.mjs";

export { default } from "./build/index.js";
// The SDK asks the entry to export this too (its type declarations, `ContainerProxy`).
export { ContainerProxy } from "@cloudflare/sandbox";

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
