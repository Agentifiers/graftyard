// F80 (`decisions/DECISIONS.md`, `f80-alarm-start`): a hosted runner's Sandbox starts its own process from its alarm,
// so a delivery pass cut at `waitUntil`'s 30 s never leaves a registered runner with no process. Run with
// `node --test workers/runners/tests/`. A fake Sandbox on virtual time: a call that needs the container waits for
// placement up to the SDK's transport budget, then throws "no container instance"; every call belongs to an
// invocation, a delivery pass's (30 s) or an alarm's (15 min), and a call past its invocation's end cuts it, after
// which it does nothing more. An alarm whose invocation is cut runs again (at-least-once). Placement waits are BF-60's
// measured cold burst (`tests/m4/raw/wp4/f78-bf60/sandbox-starts.json`).
import { test } from "node:test";
import assert from "node:assert/strict";
import { arm, close, launch } from "../launch.mjs";

const NO_INSTANCE = "There is no container instance that can be provided to this Durable Object, try again later";
const TRANSPORT_MS = 150_000; // the SDK's 503 retry budget (`computeRetryTimeoutMs`), then it throws
const WAIT_UNTIL_MS = 30_000;
const ALARM_MS = 15 * 60_000;
const HOSTED_IDLE_MS = 600_000;
const S = 1_000;
const BURST_S = [0, 0, 0, 0, 0, 40, 48, 61, 71, 74, 81, 82, 83, 86, 94, 99, 105, 105, 110, 155];
const OPTS = { processId: "runner", cwd: "/home/runner", env: { JIT: "jit-secret" } };

class Cut extends Error {}

// Virtual time: `until` runs every timer due by `t`, in order, letting promise chains settle between them. Timers
// left at the end never fire, so each Sandbox's alarm loop stops with the test.
class Sim {
  now = 0;
  timers = [];
  constructor() {
    Date.now = () => this.now;
  }
  sleep(ms) {
    return new Promise((r) => this.timers.push({ at: this.now + ms, r }));
  }
  async until(t) {
    for (let steps = 0; ; steps++) {
      assert.ok(steps < 1_000_000, "the simulation did not settle");
      await new Promise((r) => setImmediate(r));
      this.timers.sort((a, b) => a.at - b.at);
      const n = this.timers[0];
      if (!n || n.at > t) {
        this.now = Math.max(this.now, t);
        return;
      }
      this.timers.shift();
      this.now = n.at;
      n.r();
    }
  }
}

// One Sandbox object: its storage, its `schedule` rows, its container, and an alarm loop that runs due rows one
// alarm at a time, as the Containers base class does.
class Box {
  rows = [];
  store = new Map();
  proc = null;
  starts = 0;
  errors = [];
  constructor(sim, placeS, { crashAfterStart = 0 } = {}) {
    Object.assign(this, { sim, placeAt: placeS * S, crashAfterStart });
    this.loop();
  }
  view(inv) {
    const b = this;
    const live = () => {
      if (inv.cut || b.sim.now > inv.end) {
        inv.cut = true;
        throw new Cut();
      }
    };
    const container = async () => {
      live();
      if (b.sim.now < b.placeAt) {
        await b.sim.sleep(Math.min(b.placeAt - b.sim.now, TRANSPORT_MS));
        live();
        if (b.sim.now < b.placeAt) throw new Error(NO_INSTANCE);
      }
      await b.sim.sleep(200);
      live();
    };
    return {
      ctx: {
        storage: {
          get: async (k) => (live(), structuredClone(b.store.get(k))),
          put: async (k, v) => (live(), void b.store.set(k, structuredClone(v))),
        },
      },
      schedule: async (s, cb) => (live(), void b.rows.push({ at: b.sim.now + s * S, cb })),
      getProcess: async (id) => (await container(), b.proc?.id === id ? b.proc : null),
      startProcess: async (cmd, o) => {
        await container();
        b.starts++;
        b.proc = { id: o.processId, cmd };
        if (b.crashAfterStart-- > 0) {
          inv.cut = true;
          throw new Cut();
        }
        return b.proc;
      },
      destroy: async () => (live(), void (b.proc = null)),
    };
  }
  loop() {
    (async () => {
      for (;;) {
        await this.sim.sleep(S);
        for (const row of this.rows.filter((r) => r.at <= this.sim.now)) {
          try {
            await launch(this.view({ end: this.sim.now + ALARM_MS }));
            this.rows.splice(this.rows.indexOf(row), 1);
          } catch (e) {
            if (e instanceof Cut) row.at = this.sim.now + 2 * S;
            else this.rows.splice(this.rows.indexOf(row), 1), this.errors.push(e);
          }
        }
      }
    })();
  }
  // A delivery pass's call: `arm` under `waitUntil`'s 30 s. Returns whether it armed, or "cut".
  async deliver() {
    try {
      return await arm(this.view({ end: this.sim.now + WAIT_UNTIL_MS }), "run.sh --jitconfig \"$JIT\"", OPTS);
    } catch (e) {
      if (e instanceof Cut) return "cut";
      throw e;
    }
  }
  // `graft-runners`' destroy RPC (`entry.mjs`): close, then destroy the container.
  async destroy() {
    const sb = this.view({ end: Infinity });
    await close(sb);
    await sb.destroy();
  }
  state() {
    return this.store.get("runner");
  }
}

test("a cold burst of 20 starts every runner's process once, though no delivery pass outlives 30 s", { timeout: 20_000 }, async () => {
  const sim = new Sim();
  const boxes = BURST_S.map((p) => new Box(sim, p));
  const passes = [];
  for (const b of boxes) {
    passes.push(b.deliver()); // each pass runs alongside the others, as `waitUntil` does
    await sim.until(sim.now + 650); // 20 addresses over 12.9 s, as run 1966284's burst
  }
  await sim.until(15 * 60 * S);
  const armed = await Promise.all(passes);
  assert.deepEqual(armed, boxes.map(() => true), "every delivery pass armed and returned, none cut at 30 s");
  assert.deepEqual(boxes.map((b) => b.starts), boxes.map(() => 1), "each runner's process started exactly once");
  assert.ok(boxes.every((b) => b.proc), "every runner is running");
  for (const b of boxes) {
    assert.deepEqual(b.state(), { state: "launched" }, "the JIT config is dropped once the process is up");
    assert.deepEqual(b.errors, []);
  }
});

test("a retried alarm and a second delivery for the same runner start no second process", { timeout: 20_000 }, async () => {
  const sim = new Sim();
  const b = new Box(sim, 5, { crashAfterStart: 1 });
  assert.equal(await b.deliver(), true);
  await sim.until(60 * S);
  assert.equal(b.starts, 1, "the alarm retried after its crash found the process");
  assert.deepEqual(b.state(), { state: "launched" });
  // The runner exits and its container stops; a late delivery for it arms nothing.
  b.proc = null;
  assert.equal(await b.deliver(), false);
  await sim.until(5 * 60 * S);
  assert.equal(b.starts, 1, "the single-use JIT config was never started twice");
});

test("a destroy while the start waits on placement, or before it, leaves no process", { timeout: 20_000 }, async () => {
  const sim = new Sim();
  const waiting = new Box(sim, 60);
  const early = new Box(sim, 0);
  await waiting.deliver();
  await early.deliver();
  await early.destroy(); // before its alarm runs
  await sim.until(20 * S);
  await waiting.destroy(); // while its alarm waits for an instance
  await sim.until(5 * 60 * S);
  assert.equal(early.starts, 0, "a destroyed Sandbox's alarm starts nothing");
  assert.equal(waiting.proc, null, "a start that lands after the destroy is destroyed too");
  for (const b of [waiting, early]) {
    assert.deepEqual(b.state(), { state: "closed" });
    assert.deepEqual(b.errors, []);
  }
});

test("a Sandbox never placed gives up before HOSTED_IDLE and starts nothing after", { timeout: 20_000 }, async () => {
  const sim = new Sim();
  const b = new Box(sim, Infinity);
  assert.equal(await b.deliver(), true);
  await sim.until(HOSTED_IDLE_MS);
  assert.deepEqual(b.state(), { state: "closed" }, "closed, with the JIT config dropped, before the runner goes stale");
  assert.deepEqual(b.rows, [], "no start is left scheduled");
  b.placeAt = sim.now; // placement arrives once the install has deleted the runner
  await sim.until(HOSTED_IDLE_MS + 5 * 60 * S);
  assert.equal(b.starts, 0);
});
