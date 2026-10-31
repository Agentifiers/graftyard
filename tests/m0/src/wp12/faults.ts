// M0-WP12 copy of src/faults.ts (copied 2026-10-30), with `re`: a rule may match its label by regular expression, so
// one plan can fault one group's calls (labels here are "<group>|<caller>><callee>:<method>"). The FaultControl object
// lives in graft-wp12-load and is named "wp12", never "plan".
//
// M0 fault injector for member calls and storage writes (build/BUILD-PLAN.md, M0-WP1 tools; used by M0-WP2's
// B0-03, B0-11, and B0-25). A member wraps its peer stubs with `faultyStub` and its storage writes with `faultyWrite`.
// The plan lives in one `FaultControl` object in graft-m0-tools, which a member binds with `script_name`, so a plan
// changes mid-run without a new site-script version. Each isolate rereads the plan at most once a second.

import { DurableObject } from "cloudflare:workers";

export type FaultMode =
  | "error" // the call fails before it reaches the callee (a failing member class, B0-03 and B0-25)
  | "hang" // the call never reaches the callee and never returns within hangMs (a blackholed class, B0-03)
  | "drop-reply" // the callee runs the call and the reply is lost (B0-11's dropped calls)
  | "slow-reply"; // the callee runs the call and the reply arrives hangMs later (a slow link back to the caller)

export interface FaultRule {
  /** `call` is a member-to-member call; `write` is a storage write, which resets the object as a failed write does. */
  target: "call" | "write";
  mode?: FaultMode; // calls only; a write fault always resets the object
  /** Share of matching operations hit, from 0 to 1. */
  rate: number;
  /** Matches when the label contains this. Call labels are "caller>callee:method", such as "edge>a:commit" or
   *  "b>a:append", so ">a:" is every call to site a and "a>" every call from it. Write labels are "site:what". */
  match?: string;
  /** M0-WP12: a regular expression the label must match, in addition to `match`. */
  re?: string;
  hangMs?: number;
}

/** `from` and `until` bound the plan in epoch ms. A plan set with `from` a few seconds ahead reaches every isolate's
 *  cache before it starts, so all callers see the fault begin at the same moment. */
export interface FaultPlan { rules: FaultRule[]; from?: number; until?: number }

export interface FaultStats { calls: number; writes: number; injected: Record<string, number> }

export class InjectedFault extends Error {
  constructor(what: string) { super(`m0 injected fault: ${what}`); this.name = "InjectedFault"; }
}

const EMPTY: FaultPlan = { rules: [] };
const reCache = new Map<string, RegExp>();
const reOf = (s: string) => { let r = reCache.get(s); if (!r) { r = new RegExp(s); reCache.set(s, r); } return r; };
const PLAN_TTL_MS = 1000;
const STALE_MS = 3000;

/** Holds the run's plan. One object, named "plan". */
export class FaultControl extends DurableObject {
  async set(plan: FaultPlan): Promise<void> { await this.ctx.storage.put("plan", plan); }
  async get(): Promise<FaultPlan> { return (await this.ctx.storage.get<FaultPlan>("plan")) ?? EMPTY; }
}

/** One per isolate. `control` is the FaultControl namespace the member binds. */
export class Faults {
  private plan: FaultPlan = EMPTY;
  private fetchedAt = 0;
  readonly stats: FaultStats = { calls: 0, writes: 0, injected: {} };
  /** `planName` is the FaultControl object this deployment reads: "plan" for graft-raft-* and graft-m0-tools, its own
   *  name for another deployment (M0-WP4's graft-wp4-*), so its faults never reach a run on the other scripts. */
  constructor(private control: DurableObjectNamespace<FaultControl> | undefined, private random = Math.random, private planName = "plan") {}

  private refreshing: Promise<void> | null = null;

  /** The cached plan. A stale plan refreshes in the background, so the plan's own object, which may be far away,
   *  never sits on the measured path; only an isolate's first roll waits for it. */
  private async current(): Promise<FaultPlan> {
    if (!this.control) return EMPTY;
    if (Date.now() - this.fetchedAt > PLAN_TTL_MS && !this.refreshing) {
      // M0-WP12: a plan older than STALE_MS is awaited, not refreshed in the background, so a member that has made no
      // call for a while (a follower's first vote in a stall) never acts on an old plan.
      const stale = Date.now() - this.fetchedAt > STALE_MS;
      this.refreshing = this.control.getByName(this.planName).get()
        .then((p) => { this.plan = p; }, () => { /* keep the last plan */ })
        .finally(() => { this.fetchedAt = Date.now(); this.refreshing = null; });
      if (this.fetchedAt === 0 || stale) await this.refreshing;
    }
    const now = Date.now();
    return (this.plan.from !== undefined && now < this.plan.from) || (this.plan.until !== undefined && now > this.plan.until) ? EMPTY : this.plan;
  }

  /** The first matching rule that fires for this operation, if any. Each matching rule rolls on its own, so two rules
   *  at 0.1 hit 0.1 and 0.09 of operations. */
  async roll(target: FaultRule["target"], label: string): Promise<FaultRule | undefined> {
    await this.current();
    return this.rollNow(target, label);
  }

  /** As `roll`, from the cached plan with no await, for a write that must happen in the same synchronous step as the
   *  decision before it. A stale plan refreshes in the background. */
  rollNow(target: FaultRule["target"], label: string): FaultRule | undefined {
    if (Date.now() - this.fetchedAt > PLAN_TTL_MS) void this.current();
    this.stats[target === "call" ? "calls" : "writes"]++;
    const now = Date.now();
    const plan = (this.plan.from !== undefined && now < this.plan.from) || (this.plan.until !== undefined && now > this.plan.until) ? EMPTY : this.plan;
    const rule = plan.rules.find((r) =>
      r.target === target && label.includes(r.match ?? "") && (!r.re || reOf(r.re).test(label)) && this.random() < r.rate);
    if (!rule) return undefined;
    const key = `${target}:${rule.mode ?? "reset"}`;
    this.stats.injected[key] = (this.stats.injected[key] ?? 0) + 1;
    return rule;
  }

  /** For a call that starts before a cached plan's `from`: a promise that fails the call when the plan begins, as a
   *  site that is lost or blackholed also breaks calls already in flight (B0-03). Rolls once, at the call's start. */
  cutAt(label: string): { promise: Promise<never>; cancel: () => void } | undefined {
    const now = Date.now();
    if (this.plan.from === undefined || this.plan.from <= now) return undefined;
    const rule = this.plan.rules.find((r) => r.target === "call" && r.mode !== "drop-reply" && r.mode !== "slow-reply" &&
      label.includes(r.match ?? "") && (!r.re || reOf(r.re).test(label)) && this.random() < r.rate);
    if (!rule) return undefined;
    let timer: ReturnType<typeof setTimeout>;
    const promise = new Promise<never>((_, reject) => {
      timer = setTimeout(() => reject(new InjectedFault(`call ${label} ${rule.mode === "hang" ? "hung" : "failed"} in flight`)),
        this.plan.from! - now + (rule.mode === "hang" ? rule.hangMs ?? 60_000 : 0));
    });
    return { promise, cancel: () => clearTimeout(timer) };
  }
}

/** Wraps an RPC stub so every method call first rolls the plan. `label` names the callee, such as its site key. */
export function faultyStub<T extends object>(stub: T, faults: Faults, label: string): T {
  return new Proxy(stub, {
    get(target, prop, receiver) {
      const value = Reflect.get(target, prop, receiver);
      if (typeof value !== "function" || typeof prop !== "string") return value;
      return async (...args: unknown[]) => {
        const rule = await faults.roll("call", `${label}:${prop}`);
        if (rule?.mode === "hang") {
          await new Promise((r) => setTimeout(r, rule.hangMs ?? 60_000));
          throw new InjectedFault(`call ${label}:${prop} hung`);
        }
        if (rule && rule.mode !== "drop-reply" && rule.mode !== "slow-reply") throw new InjectedFault(`call ${label}:${prop} failed`);
        // Reflect.apply, because `.apply` on an RPC stub's method is itself read as a remote method named "apply".
        const pending = Reflect.apply(value as (...a: unknown[]) => unknown, target, args) as Promise<unknown>;
        const cut = faults.cutAt(`${label}:${prop}`);
        const result = cut ? await Promise.race([pending, cut.promise]).finally(cut.cancel) : await pending;
        if (rule?.mode === "slow-reply") { await new Promise((r) => setTimeout(r, rule.hangMs ?? 300)); return result; }
        if (rule) throw new InjectedFault(`call ${label}:${prop} reply dropped`);
        return result;
      };
    },
  });
}

/** Runs a storage write, unless the plan fails it. A failed write resets the object, as an unconfirmed write does
 *  (https://blog.cloudflare.com/sqlite-in-durable-objects, "the Durable Object itself will restart"), so in-memory
 *  state is lost and the caller sees an error. A reset also fails every other request in flight on that object, so the
 *  share of failed calls is higher than `rate`. The roll awaits the plan, so do every read the write depends on inside
 *  `write`, which runs synchronously after it: input gates do not hold other requests out across an outgoing call. */
export async function faultyWrite<R>(ctx: DurableObjectState, faults: Faults, label: string, write: () => R): Promise<R> {
  if (await faults.roll("write", label)) {
    ctx.abort(`m0 injected fault: write ${label}`);
    throw new InjectedFault(`write ${label}`); // abort() already throws inside workerd; this covers older runtimes
  }
  return write();
}
