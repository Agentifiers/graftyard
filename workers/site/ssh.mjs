// The SSH front's Durable Object and outbound handler (design/DESIGN.md, Shape, "SSH front"; build/BUILD-PLAN.md,
// M6-WP2), in JavaScript because the container's TCP port, its outbound intercepts, and `ctx.exports` have no
// worker-rs binding. `entry.mjs` exports them beside the Rust classes.
//
// - `SshFront`: one object per site (`ssh#<site>`), which the edge's `connect()` handler opens with `stub.connect`. It
//   starts its `sshfront` container (`graft-sshd`) with no Internet and two intercepts, `keys.graft` and `git.graft`,
//   both to `SshOutbound`, then pipes the socket to the container's port 22. It sends the client nothing until
//   `graft-sshd`'s banner arrives, so the edge can still try another site's front: a cold container's first
//   connection closes with no bytes (B0-24d, tests/m6/results/m6-wp2.md), and a connection is retried until the banner
//   comes or BANNER_WAIT_MS ends. A running container on another image than this version's `ssh` one is restarted at
//   the next session (`up`), and `status` answers the object's `GET /status`: running, both images, and the banner.
// - `SshOutbound`: `keys.graft/slots` answers both host-key slots from Secrets Store and the active one from the
//   `Release` view, through the script's own `SITE` binding (`src/lib.rs`, `ssh_active`), and `git.graft` forwards to the
//   edge over the `EDGE` service binding, whose `props` (`{"ssh": true}`) are what tells the edge the call is the
//   front's. The container never holds a secret or reaches the public zone.
//
// `o` is the object: `ctx.container`, `ctx.storage`, `ctx.exports`, and `sleep(ms)`.

export const BANNER_WAIT_MS = 45_000; // [guess]: a cold start took about 2 s (B0-24d), and placement can be slower
export const RETRY_MS = 500; // [guess]
// How long `status` waits for a running front's banner [guess]: under graft-ops' 2 s `VIEW_CALL` (workers/ops/src/exec.rs),
// so a slow banner reads as `banner: null`, which the probe refuses by name, not as a call that did not answer.
export const STATUS_WAIT_MS = 1_500;
// The container's inactivity timeout: an hour without a session [guess], so a burst of sessions finds it warm. The
// timer starts when the object goes inactive, which is when its last session's `connect()` ends: the object keeps no
// promise past a session. A pending `monitor()` held it up to 15 minutes longer [doc: Durable Object Container API,
// `monitor`], which kept B0-24d's fronts up 1 h 15 min after their last session (tests/m6/results/m6-wp2.md).
export const IDLE_MS = 3_600_000;
export const INSTANCE = "basic";

// Starts the container if it is not running, with its intercepts registered first, as the platform asks. A container
// that stopped (a crash, the inactivity timeout, a rollout) is started again by the next session, which waits for the
// banner (`open`, a cold start of 2 to 6 s in B0-24d): the object needs no alarm and no `monitor()`, whose pending
// promise would hold it past the session.
//
// A new version restarts no running container, so a running one whose image (`storage` key `image`, written at each
// start) is not this version's `ssh` image, or that an object from before this key started, is destroyed and started
// on this version's image, as `GitShard` does (`src/shard.rs`, `restart_on_image`). graft-m3f's sites a and b kept a
// graft-sshd that read the host-key slot once at start (m3fs1). A new version already resets the object, which ends
// its in-flight sessions, so restarting at the first session on the new version costs no extra session [inference].
// A start that throws while the old container is still stopping is retried by `open`.
export async function up(o) {
  const c = o.ctx.container;
  const image = c.images?.ssh;
  if (c.running) {
    const ran = await o.ctx.storage.get("image");
    if (ran === image) return;
    console.log(`SshFront: image ${ran ?? "unrecorded"} -> ${image}: restarting`);
    await c.destroy("sshfront: this version names another image");
  }
  await c.interceptOutboundHttp("keys.graft", o.ctx.exports.SshOutbound({ props: { host: "keys" } }));
  await c.interceptOutboundHttp("git.graft", o.ctx.exports.SshOutbound({ props: { host: "git" } }));
  if (!image) throw new Error('no image "ssh" in this version\'s image map');
  c.start({ image, instance: INSTANCE, enableInternet: false });
  await o.ctx.storage.put("image", image);
  await c.setInactivityTimeout?.(IDLE_MS);
}

// One connection to graft-sshd that has sent its banner: the socket and the first bytes it sent.
export async function open(o, deadline = Date.now() + BANNER_WAIT_MS) {
  let last;
  for (;;) {
    try {
      await up(o);
      const s = o.ctx.container.getTcpPort(22).connect("10.0.0.1:22");
      await s.opened;
      const r = s.readable.getReader();
      const { value, done } = await r.read();
      r.releaseLock();
      if (!done && value?.byteLength) return { socket: s, first: value };
      last = new Error("graft-sshd closed before its banner");
      s.close?.();
    } catch (e) {
      last = e;
    }
    if (Date.now() >= deadline) throw new Error(`no SSH banner by the deadline: ${last?.message ?? last}`);
    await o.sleep(RETRY_MS);
  }
}

// The object's `connect(socket)`: the banner first, then both directions until either side closes.
export async function connect(o, socket, wait = BANNER_WAIT_MS) {
  let front;
  try {
    front = await open(o, Date.now() + wait);
  } catch (e) {
    console.error(`SshFront: ${e.message}`);
    socket.close?.();
    return;
  }
  const w = socket.writable.getWriter();
  await w.write(front.first);
  w.releaseLock();
  await Promise.allSettled([socket.readable.pipeTo(front.socket.writable), front.socket.readable.pipeTo(socket.writable)]);
}

// `GET /status` on the object, which this site's Registry serves at `/ssh_front` and graft-ops' `SshHostKeys` probe
// reads (`workers/ops/src/lib.rs`, `ssh_fronts_ready`): whether the container runs, the image this object last started
// and this version's, and the banner graft-sshd sends on one connection, whose version names its generation. It never
// starts the container: a stopped one starts on this version's image at its next session.
export async function status(o) {
  const c = o.ctx.container;
  const out = { running: !!c.running, image: (await o.ctx.storage.get("image")) ?? null, version_image: c.images?.ssh ?? null, banner: null };
  if (!out.running) return out;
  let timer;
  let s;
  try {
    s = c.getTcpPort(22).connect("10.0.0.1:22");
    const read = (async () => {
      await s.opened;
      const r = s.readable.getReader();
      const { value } = await r.read();
      r.releaseLock();
      return value;
    })();
    read.catch(() => {}); // a read the close below ends after the deadline won is not an unhandled rejection
    const late = new Promise((r) => (timer = setTimeout(r, STATUS_WAIT_MS)));
    const value = await Promise.race([read, late]);
    if (value?.byteLength) out.banner = new TextDecoder().decode(value).split("\n")[0].replace(/\r$/, "");
  } catch {
    // No banner: the container is starting or stopping.
  } finally {
    clearTimeout(timer);
    s?.close?.();
  }
  return out;
}

// `SshOutbound`'s fetch: `props.host` is the intercept it serves.
export async function outbound(props, env, request) {
  const url = new URL(request.url);
  if (props?.host === "keys") {
    if (url.pathname !== "/slots" || request.method !== "GET") return new Response("Not Found", { status: 404 });
    // The active slot is the `Release` view's (`/_graftyard/ssh/active` on this script's own fetch handler), so a
    // committed `Rotate { SshHostKeys }` switch is served with no new version. Without it, 503: graft-sshd retries.
    if (!env.SITE) return new Response("this site has no SITE binding", { status: 503 });
    const a = await env.SITE.fetch("https://site/_graftyard/ssh/active");
    if (a.status !== 200) return new Response(`the active host-key slot did not answer (HTTP ${a.status})`, { status: 503 });
    const { active } = await a.json();
    if (active !== "A" && active !== "B") return new Response(`no host-key slot ${active}`, { status: 503 });
    const slot = async (b) => (b ? await b.get() : "");
    const slots = { A: await slot(env.SSH_HOST_KEY_A), B: await slot(env.SSH_HOST_KEY_B) };
    return Response.json({ active, slots });
  }
  if (props?.host === "git") {
    if (!env.EDGE) return new Response("this site has no EDGE binding", { status: 503 });
    return env.EDGE.fetch(new Request(`https://git.graft${url.pathname}${url.search}`, request), { redirect: "manual" });
  }
  return new Response("Forbidden", { status: 403 });
}
