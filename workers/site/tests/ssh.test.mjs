// The SSH front's object and outbound handler (`workers/site/ssh.mjs`). Run with
// `node --test 'workers/site/tests/*.test.mjs'`. A fake `ctx.container` whose port 22 behaves as B0-24d measured: the
// first connections to a cold container close with no bytes, later ones send graft-sshd's banner.
import { test } from "node:test";
import assert from "node:assert/strict";
import * as ssh from "../ssh.mjs";

// A socket whose readable yields `chunks` and whose writable records what it is given.
function socket(chunks) {
  const written = [];
  return {
    written,
    opened: Promise.resolve({}),
    readable: new ReadableStream({
      start(c) {
        for (const x of chunks) c.enqueue(new TextEncoder().encode(x));
        c.close();
      },
    }),
    writable: new WritableStream({ write: (b) => written.push(new TextDecoder().decode(b)) }),
    close() {},
  };
}

function object({ coldTries = 2, image = "registry/sshfront@sha256:1" } = {}) {
  const intercepts = [];
  const starts = [];
  const destroys = [];
  let tries = 0;
  const container = {
    running: false,
    images: image ? { ssh: image } : {},
    async interceptOutboundHttp(host, target) {
      intercepts.push({ host, props: target.props, running: this.running });
    },
    start(o) {
      starts.push(o);
      this.running = true;
    },
    async destroy(reason) {
      destroys.push(reason);
      this.running = false;
    },
    timeout: 0,
    async setInactivityTimeout(ms) {
      this.timeout = ms;
    },
    // Settles only when the container stops, which no test does, so each call stays pending.
    monitors: 0,
    monitor() {
      this.monitors++;
      return new Promise(() => {});
    },
    getTcpPort(p) {
      assert.equal(p, 22);
      return { connect: () => (++tries <= coldTries ? socket([]) : socket(["SSH-2.0-graftyard\r\n", "rest"])) };
    },
  };
  let alarm = null;
  const o = {
    ctx: {
      container,
      storage: { m: new Map(), async put(k, v) { this.m.set(k, v); }, async get(k) { return this.m.get(k); }, async setAlarm(t) { alarm = t; } },
      exports: { SshOutbound: ({ props }) => ({ props }) },
    },
    slept: 0,
    async sleep(ms) {
      this.slept += ms;
    },
  };
  return { o, intercepts, starts, destroys, tries: () => tries, alarm: () => alarm };
}

test("a cold front is retried until graft-sshd's banner, which reaches the client first, then the rest", async () => {
  const f = object({ coldTries: 3 });
  const client = socket(["SSH-2.0-OpenSSH_10.6\r\n"]);
  await ssh.connect(f.o, client);
  assert.equal(f.tries(), 4);
  assert.deepEqual(client.written, ["SSH-2.0-graftyard\r\n", "rest"]);
  // Both intercepts registered before the start, with no Internet, on the image map's `ssh` image, once.
  assert.deepEqual(f.intercepts.map((i) => [i.host, i.props.host, i.running]), [["keys.graft", "keys", false], ["git.graft", "git", false]]);
  assert.equal(f.starts.length, 1);
  assert.deepEqual(f.starts[0], { image: "registry/sshfront@sha256:1", instance: "basic", enableInternet: false });
});

test("a front that never sends its banner closes the client with nothing written", async () => {
  const f = object({ coldTries: Infinity });
  const client = socket([]);
  let closed = false;
  client.close = () => (closed = true);
  await ssh.connect(f.o, client, 50);
  assert.ok(closed);
  assert.deepEqual(client.written, []);
  assert.ok(f.tries() > 1, "retried before giving up");
});

test("a version with no ssh image refuses to start and says why", async () => {
  const f = object({ image: null });
  await assert.rejects(ssh.open(f.o, 0), /no image "ssh"/);
});

// The script's own `SITE` binding, whose `/_graftyard/ssh/active` answers `status` with the `Release` view's slot.
function site(active, status = 200) {
  const calls = [];
  return {
    calls,
    async fetch(url) {
      calls.push(String(url));
      return status === 200 ? Response.json({ active }) : new Response("no Release view yet", { status });
    },
  };
}

test("SshOutbound serves the host-key slots on keys.graft and forwards git.graft to EDGE only", async () => {
  const env = {
    SSH_HOST_KEY_A: { get: async () => "A-PEM" },
    SSH_HOST_KEY_B: { get: async () => "B-PEM" },
    SITE: site("A"),
    EDGE: { fetch: async (r) => new Response(`edge saw ${new URL(r.url).pathname}${new URL(r.url).search} ${r.headers.get("x-graftyard-ssh-key")}`) },
  };
  const keys = await ssh.outbound({ host: "keys" }, env, new Request("http://keys.graft/slots"));
  assert.deepEqual(await keys.json(), { active: "A", slots: { A: "A-PEM", B: "B-PEM" } });
  assert.deepEqual(env.SITE.calls, ["https://site/_graftyard/ssh/active"]);
  assert.equal((await ssh.outbound({ host: "keys" }, env, new Request("http://keys.graft/other"))).status, 404);
  const req = new Request("http://git.graft/o/r.git/info/refs?service=git-upload-pack", { headers: { "x-graftyard-ssh-key": "ab" } });
  assert.equal(await (await ssh.outbound({ host: "git" }, env, req)).text(), "edge saw /o/r.git/info/refs?service=git-upload-pack ab");
  assert.equal((await ssh.outbound({}, env, new Request("http://elsewhere/"))).status, 403);
  assert.equal((await ssh.outbound({ host: "git" }, { ...env, EDGE: undefined }, req)).status, 503);
});

test("keys.graft's active slot is the Release view's: slot B after a Rotate switch, with no variable", async () => {
  const env = { SSH_HOST_KEY_A: { get: async () => "A-PEM" }, SSH_HOST_KEY_B: { get: async () => "B-PEM" }, SITE: site("B") };
  const keys = await ssh.outbound({ host: "keys" }, env, new Request("http://keys.graft/slots"));
  assert.equal(keys.status, 200);
  assert.deepEqual(await keys.json(), { active: "B", slots: { A: "A-PEM", B: "B-PEM" } });
});

test("keys.graft answers 503 while the active slot is unknown, so graft-sshd retries rather than guess", async () => {
  const keysEnv = { SSH_HOST_KEY_A: { get: async () => "A-PEM" }, SSH_HOST_KEY_B: { get: async () => "B-PEM" } };
  const req = () => new Request("http://keys.graft/slots");
  // The view has no entries yet.
  assert.equal((await ssh.outbound({ host: "keys" }, { ...keysEnv, SITE: site(null, 503) }, req())).status, 503);
  // No `SITE` binding.
  assert.equal((await ssh.outbound({ host: "keys" }, keysEnv, req())).status, 503);
});

// When the platform stops the container after its last session, from the session's end: the object goes inactive
// only once nothing holds it, a pending `monitor()` holds it up to 15 minutes and an alarm wakes it [doc: Durable
// Object Container API, `monitor` and `setInactivityTimeout`], and the container then runs its inactivity timeout.
const MONITOR_HOLD_MS = 15 * 60_000;
function stopsAfter(f, sessionEnd) {
  const c = f.o.ctx.container;
  const inactive = Math.max(sessionEnd, c.monitors ? sessionEnd + MONITOR_HOLD_MS : 0, f.alarm() ?? 0);
  return inactive + c.timeout - sessionEnd;
}

test("a front's container stops within IDLE_MS of its last session's end, with nothing holding the object", async () => {
  // B0-24d: fronts whose last session ended at 05:01Z stopped at 06:16Z and 06:17Z, an hour and 15 minutes later.
  const f = object({ coldTries: 1 });
  await ssh.connect(f.o, socket(["SSH-2.0-OpenSSH_10.6\r\n"]));
  await ssh.connect(f.o, socket(["SSH-2.0-OpenSSH_10.6\r\n"]));
  const end = Date.now();
  assert.equal(f.starts.length, 1, "the second session reused the running container");
  assert.equal(f.o.ctx.container.timeout, ssh.IDLE_MS);
  assert.ok(stopsAfter(f, end) <= ssh.IDLE_MS, `stops ${stopsAfter(f, end) / 60_000} min after the session`);
});

test("a running front restarts once on a version that names another image, and on one no object recorded", async () => {
  const f = object({ coldTries: 0, image: "registry/sshfront@sha256:X" });
  const c = f.o.ctx.container;
  const session = () => ssh.connect(f.o, socket(["SSH-2.0-OpenSSH_10.6\r\n"]));
  await session();
  await session();
  assert.equal(f.starts.length, 1, "the same image keeps the running container");
  assert.deepEqual(f.destroys, []);
  assert.equal(await f.o.ctx.storage.get("image"), "registry/sshfront@sha256:X");
  // A new version whose image map names another `ssh` image.
  c.images = { ssh: "registry/sshfront@sha256:Y" };
  await session();
  await session();
  assert.deepEqual(f.destroys, ["sshfront: this version names another image"]);
  assert.deepEqual(f.starts.map((s) => s.image), ["registry/sshfront@sha256:X", "registry/sshfront@sha256:Y"]);
  assert.equal(await f.o.ctx.storage.get("image"), "registry/sshfront@sha256:Y");
  // The restart registers the intercepts again before its start.
  assert.deepEqual(f.intercepts.slice(2).map((i) => [i.host, i.running]), [["keys.graft", false], ["git.graft", false]]);

  // A container an object from before the `image` key started: running, nothing recorded.
  const old = object({ coldTries: 0, image: "registry/sshfront@sha256:Y" });
  old.o.ctx.container.running = true;
  await ssh.connect(old.o, socket(["SSH-2.0-OpenSSH_10.6\r\n"]));
  await ssh.connect(old.o, socket(["SSH-2.0-OpenSSH_10.6\r\n"]));
  assert.equal(old.destroys.length, 1);
  assert.deepEqual(old.starts.map((s) => s.image), ["registry/sshfront@sha256:Y"]);
  assert.equal(await old.o.ctx.storage.get("image"), "registry/sshfront@sha256:Y");
});

test("status answers a running front's banner without CR/LF and never starts a stopped one", async () => {
  const f = object({ coldTries: 0, image: "registry/sshfront@sha256:Y" });
  assert.deepEqual(await ssh.status(f.o), { running: false, image: null, version_image: "registry/sshfront@sha256:Y", banner: null });
  assert.equal(f.starts.length, 0);
  assert.equal(f.tries(), 0, "a stopped front is not dialled");
  await ssh.connect(f.o, socket(["SSH-2.0-OpenSSH_10.6\r\n"]));
  assert.deepEqual(await ssh.status(f.o), {
    running: true,
    image: "registry/sshfront@sha256:Y",
    version_image: "registry/sshfront@sha256:Y",
    banner: "SSH-2.0-graftyard",
  });
  assert.equal(f.starts.length, 1, "status started nothing");
  assert.deepEqual(f.destroys, []);
});
