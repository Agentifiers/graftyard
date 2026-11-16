// `graft-runners`' entry (workers/runners/deploy.mjs): the Rust Worker's handlers, which `worker-build` writes to its build folder,
// and the Sandbox SDK's Durable Object class, once per instance type, because a container class has one instance type.
// `worker-build` exports only Rust classes, so the SDK's JS class is exported here and wrangler bundles both.
import { Sandbox } from "@cloudflare/sandbox";

export { default } from "./build/index.js";
// The SDK asks the entry to export this too (its type declarations, `ContainerProxy`).
export { ContainerProxy } from "@cloudflare/sandbox";
export class SandboxStd3 extends Sandbox {}
export class SandboxStd4 extends Sandbox {}
