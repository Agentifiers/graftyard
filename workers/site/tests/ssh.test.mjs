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
    async setInactivityTimeout() {},
    monitor: () => new Promise(() => {}),
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
  return { o, intercepts, starts, tries: () => tries, alarm: () => alarm };
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

test("SshOutbound serves the host-key slots on keys.graft and forwards git.graft to EDGE only", async () => {
  const env = {
    SSH_HOST_KEY_A: { get: async () => "A-PEM" },
    SSH_HOST_KEY_B: { get: async () => "B-PEM" },
    SSH_HOST_KEY_ACTIVE: "B",
    EDGE: { fetch: async (r) => new Response(`edge saw ${new URL(r.url).pathname}${new URL(r.url).search} ${r.headers.get("x-graftyard-ssh-key")}`) },
  };
  const keys = await ssh.outbound({ host: "keys" }, env, new Request("http://keys.graft/slots"));
  assert.deepEqual(await keys.json(), { active: "B", slots: { A: "A-PEM", B: "B-PEM" } });
  assert.equal((await ssh.outbound({ host: "keys" }, env, new Request("http://keys.graft/other"))).status, 404);
  const req = new Request("http://git.graft/o/r.git/info/refs?service=git-upload-pack", { headers: { "x-graftyard-ssh-key": "ab" } });
  assert.equal(await (await ssh.outbound({ host: "git" }, env, req)).text(), "edge saw /o/r.git/info/refs?service=git-upload-pack ab");
  assert.equal((await ssh.outbound({}, env, new Request("http://elsewhere/"))).status, 403);
  assert.equal((await ssh.outbound({ host: "git" }, { ...env, EDGE: undefined }, req)).status, 503);
});

test("the alarm restarts a stopped container only while sessions came within IDLE_MS", async () => {
  const f = object();
  await f.o.ctx.storage.put("seen", Date.now());
  await ssh.alarm(f.o);
  assert.equal(f.starts.length, 1);
  const g = object();
  await g.o.ctx.storage.put("seen", Date.now() - ssh.IDLE_MS - 1);
  await ssh.alarm(g.o);
  assert.equal(g.starts.length, 0);
});
