// R23 (`decisions/DECISIONS.md`, `r23-runnerbox`): a hosted runner's `RunnerBox` starts its own container from its
// alarm with the platform's `ctx.container`, on the image of the Worker version that runs the object. Run with
// `node --test 'workers/runners/tests/*.test.mjs'`. A fake account on virtual time: each object has storage, an
// alarm, and a container whose start places it only while the account has vCPU left (else the monitor rejects with
// "no container instance", as the runtime answers a refused placement [unverified which call surfaces it natively;
// `box.mjs` handles both]). The container runs its entrypoint as PID 1, so it stops when that process exits.
import { test } from "node:test";
import assert from "node:assert/strict";
import * as box from "../box.mjs";

const NO_INSTANCE = "There is no container instance that can be provided to this Durable Object, try again later";
const S = 1_000;
const VCPU = { "standard-3": 2, "standard-4": 4 };
const RUN_SH = 'exec /home/runner/run.sh --jitconfig "$JIT"';

class Cut extends Error {}

// Every line `box.mjs` logs, in order; each names its image.
const logs = [];
console.log = console.error = (m) => logs.push(String(m));

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

// The runner account: its vCPU limit, the containers running now, and the most that ever ran at once.
class Account {
  used = 0;
  peak = 0;
  constructor(sim, vcpu = 15_000) {
    Object.assign(this, { sim, vcpu });
  }
}

// One `RunnerBox`, run by one Worker version (its image map). `image` says what a container of each image does:
// "ok" runs a job of `jobS` then exits, "broken" exits at once.
class Obj {
  store = new Map();
  alarmAt = null;
  starts = []; // every placed start: { image, instance, pid1, jit }
  tries = 0;
  c = null; // the running container, { vcpu, exit }
  constructor(sim, account, images, { jobS = 60, broken = [], crashAfterStart = 0 } = {}) {
    Object.assign(this, { sim, account, images, jobS, broken, crashAfterStart });
    this.loop();
  }
  stop() {
    if (!this.c) return;
    this.account.used -= this.c.vcpu;
    const exit = this.c.exit;
    this.c = null;
    exit();
  }
  view(inv = {}) {
    const o = this;
    const container = {
      get running() {
        return o.c !== null;
      },
      images: o.images,
      start(opts) {
        assert.ok(!o.c, "start on a running container");
        o.tries++;
        let exit;
        const ended = new Promise((res, rej) => (exit = res) && (o.refuse = rej));
        const vcpu = VCPU[opts.instance];
        assert.ok(vcpu, `instance ${opts.instance}`);
        if (o.account.used + vcpu > o.account.vcpu) {
          // Refused placement: the monitor rejects shortly after.
          o.sim.sleep(500).then(() => o.refuse(new Error(NO_INSTANCE)));
          o.pending = ended;
          return;
        }
        o.account.used += vcpu;
        o.account.peak = Math.max(o.account.peak, o.account.used);
        o.c = { vcpu, exit };
        o.pending = ended;
        const [sh, flag, cmd] = opts.entrypoint ?? [];
        // PID 1 is the runner only when the shell `exec`s it; without an entrypoint the image's own server runs.
        const pid1 = sh === "/bin/sh" && flag === "-c" && cmd === RUN_SH ? "runner" : opts.entrypoint ? "sh" : "image";
        o.starts.push({ image: opts.image, instance: opts.instance, pid1, jit: opts.env?.JIT, internet: opts.enableInternet });
        if (pid1 !== "runner") return; // nothing ever ends this container but a destroy
        const runS = o.broken.includes(opts.image) ? 0.2 : o.jobS;
        o.sim.sleep(runS * S).then(() => o.c?.exit === exit && o.stop());
        if (o.crashAfterStart-- > 0) {
          inv.cut = true;
          throw new Cut();
        }
      },
      monitor() {
        return o.pending;
      },
      async setInactivityTimeout(ms) {
        o.inactivity = ms;
      },
      async destroy() {
        o.stop();
      },
    };
    return {
      ctx: {
        storage: {
          get: async (k) => structuredClone(o.store.get(k)),
          put: async (k, v) => void o.store.set(k, structuredClone(v)),
          setAlarm: async (t) => void (o.alarmAt = t),
        },
        container,
      },
      sleep: (ms) => o.sim.sleep(ms),
    };
  }
  loop() {
    (async () => {
      for (;;) {
        await this.sim.sleep(S);
        if (this.alarmAt === null || this.alarmAt > this.sim.now) continue;
        // The runtime clears the alarm before the handler, and runs it again if the handler is cut or throws.
        this.alarmAt = null;
        const inv = {};
        try {
          await box.alarm(this.view(inv));
        } catch (e) {
          if (!(e instanceof Cut)) throw e;
          this.alarmAt = this.sim.now + 2 * S;
        }
      }
    })();
  }
  arm(instance = "standard-3", jit = "jit-secret") {
    return box.arm(this.view(), instance, jit);
  }
  destroy() {
    return box.destroy(this.view());
  }
  state() {
    return this.store.get("runner");
  }
}

test("a version at 10% starts its own image, and the other 90% keep theirs", { timeout: 20_000 }, async () => {
  const sim = new Sim();
  const acct = new Account(sim);
  const OLD = "registry.cloudflare.com/a/runner@sha256:aaaa";
  const NEW = "registry.cloudflare.com/a/runner@sha256:bbbb";
  // A gradual deployment sends each object to one version, whose image map it sees.
  const objs = Array.from({ length: 10 }, (_, i) => new Obj(sim, acct, { runner: i === 3 ? NEW : OLD }));
  for (const o of objs) assert.equal(await o.arm(), true);
  await sim.until(30 * S);
  assert.deepEqual(
    objs.map((o) => o.starts.map((s) => s.image)),
    objs.map((_, i) => [i === 3 ? NEW : OLD]),
  );
  const launched = logs.filter((l) => l.includes("launched"));
  assert.equal(launched.filter((l) => l.endsWith(NEW)).length, 1, "the launch line names the image");
  assert.equal(launched.filter((l) => l.endsWith(OLD)).length, 9);
  for (const o of objs) assert.deepEqual(o.state(), { state: "launched", image: o.images.runner });
});

test("a broken image at 10% shows by image, and the 90% are unaffected", { timeout: 20_000 }, async () => {
  const sim = new Sim();
  const acct = new Account(sim);
  const OLD = "img@sha256:aaaa";
  const BAD = "img@sha256:dead";
  const objs = Array.from({ length: 10 }, (_, i) => new Obj(sim, acct, { runner: i === 0 ? BAD : OLD }, { broken: [BAD] }));
  logs.length = 0;
  for (const o of objs) await o.arm();
  await sim.until(30 * S);
  // The canary check counts these lines by image before the 100% step.
  const exited = logs.filter((l) => l.includes("exited within"));
  assert.deepEqual(exited, [`graft-runners: runner exited within 3 s of its start, image ${BAD}`]);
  assert.equal(logs.filter((l) => l.includes("launched") && l.endsWith(OLD)).length, 9, "the 90% launch as before");
  assert.equal(objs[0].starts.length, 1, "a broken image is started once, not looped: its JIT config is single-use");
});

test("the alarm starts the container once: placement refusals are tried again, a retried alarm starts none", { timeout: 20_000 }, async () => {
  const sim = new Sim();
  const acct = new Account(sim, 0); // no room: every start is refused
  const o = new Obj(sim, acct, { runner: "img@sha256:1" }, { crashAfterStart: 1 });
  assert.equal(await o.arm(), true);
  assert.equal(await o.arm(), false, "a second arm for the same runner stores nothing");
  await sim.until(45 * S);
  assert.ok(o.tries >= 3, `refused starts are tried again every ${box.RETRY_S} s (${o.tries})`);
  assert.equal(o.starts.length, 0);
  acct.vcpu = 15_000; // room again
  await sim.until(90 * S);
  // The first placed start's alarm was cut right after `start`; the retried alarm found it running.
  assert.equal(o.starts.length, 1, "exactly one placed start");
  assert.deepEqual(o.state(), { state: "launched", image: "img@sha256:1" }, "the JIT config is dropped");
  assert.equal(o.inactivity, box.JOB_MS);
});

test("the runner is the container's main process, so its exit stops the container", { timeout: 20_000 }, async () => {
  const sim = new Sim();
  const acct = new Account(sim);
  const o = new Obj(sim, acct, { runner: "img@sha256:1" }, { jobS: 120 });
  await o.arm("standard-4", "jit-7");
  await sim.until(20 * S);
  assert.deepEqual(o.starts, [
    { image: "img@sha256:1", instance: "standard-4", pid1: "runner", jit: "jit-7", internet: true },
  ]);
  assert.ok(!box.ENTRYPOINT.join(" ").includes("kill"), "nothing signals PID 1: the runner is PID 1");
  assert.equal(acct.used, 4);
  await sim.until(200 * S);
  assert.equal(o.c, null, "the runner exited after its job and the container stopped with it");
  assert.equal(acct.used, 0);
});

test("a cold burst past the account's vCPU never runs more than the limit; late starts land or give up", { timeout: 60_000 }, async () => {
  const sim = new Sim();
  const acct = new Account(sim, 20); // ten standard-3
  // 16 runners armed at once (a burst the pass's RUNNER_VCPU_STOP_PCT let through, e.g. from overlapping passes).
  // Jobs run 2 min: 10 place at once, 6 retry and land as the first ten exit, before GIVE_UP_MS.
  const objs = Array.from({ length: 16 }, () => new Obj(sim, acct, { runner: "img@sha256:1" }, { jobS: 120 }));
  for (const o of objs) await o.arm();
  await sim.until(20 * S);
  assert.equal(acct.used, 20, "ten placed");
  await sim.until(30 * 60 * S);
  assert.equal(acct.peak, 20, "never past the account's vCPU");
  assert.deepEqual(objs.map((o) => o.starts.length), objs.map(() => 1), "every runner started once");
  // Jobs that outlast GIVE_UP_MS: the refused ones give up, close, and are never started after that.
  const sim2 = new Sim();
  const acct2 = new Account(sim2, 4);
  logs.length = 0;
  const long = Array.from({ length: 4 }, () => new Obj(sim2, acct2, { runner: "img@sha256:1" }, { jobS: 3_600 }));
  for (const o of long) await o.arm();
  await sim2.until(20 * 60 * S);
  assert.deepEqual(long.map((o) => o.starts.length), [1, 1, 0, 0]);
  for (const o of long.slice(2)) {
    assert.deepEqual(o.state(), { state: "closed" }, "the JIT config is dropped; the registration goes stale");
  }
  assert.equal(logs.filter((l) => l.includes("gave up")).length, 2);
  await sim2.until(70 * 60 * S);
  assert.deepEqual(long.map((o) => o.starts.length), [1, 1, 0, 0], "a closed start never starts later");
});

test("a destroy wins over a start in flight, and a runner this class never held answers false", { timeout: 20_000 }, async () => {
  const sim = new Sim();
  const acct = new Account(sim, 0);
  const o = new Obj(sim, acct, { runner: "img@sha256:1" });
  const never = new Obj(sim, acct, { runner: "img@sha256:1" });
  assert.equal(await never.destroy(), false, "a runner whose start never reached this object has nothing to stop");
  assert.equal(never.state(), undefined, "and writes nothing here");
  await o.arm();
  await sim.until(25 * S); // refused, waiting to retry
  assert.equal(await o.destroy(), true);
  acct.vcpu = 15_000;
  await sim.until(5 * 60 * S);
  assert.equal(o.starts.length, 0, "a destroyed runner's alarm starts nothing");
  assert.deepEqual(o.state(), { state: "closed" });
  // A destroy while a placed start waits out PLACED_WAIT_MS: the start ends closed, with no container.
  const w = new Obj(sim, acct, { runner: "img@sha256:1" }, { jobS: 600 });
  await w.arm();
  await sim.until(sim.now + 2 * S); // the alarm has started the container and waits
  assert.equal(w.starts.length, 1);
  assert.equal(await w.destroy(), true);
  await sim.until(sim.now + 60 * S);
  assert.deepEqual(w.state(), { state: "closed" }, "the start that was waiting does not mark itself launched");
  assert.equal(w.c, null);
});
