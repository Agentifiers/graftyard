// `RunnerBox`'s start (design/DESIGN.md, Shape, `graft-runners`; decisions/DECISIONS.md, `r23-runnerbox`): one hosted
// runner per object, on the runner account's one `durable_object`-policy container application, started with the
// platform's own `ctx.container` (no Sandbox SDK), as the git cache shards start theirs (`crates/git/src/host.rs`,
// `start_once`). The image is the `runner` entry of the image map of the Worker version that runs this object, so a
// version at 10% starts its own image on the objects assigned to it [doc: containers/guides/image-management].
//
// F80's shape is kept: a pass only arms the object (`arm` stores the start and sets the object's alarm), and the alarm
// starts the container at most once, tries again every RETRY_S while placement fails, and begins no try after
// GIVE_UP_MS, so the last start lands before the install's HOSTED_IDLE (600 s). The runner is the container's main
// process (`ENTRYPOINT` execs `run.sh`), so the container stops when the runner exits. The stored JIT config is dropped
// once the start launches or closes.
//
// `o` is the object (`entry.mjs`): `ctx.storage` (get, put, setAlarm), `ctx.container` (running, images, start,
// monitor, setInactivityTimeout, destroy), and `sleep(ms)`.

export const RETRY_S = 10; // [guess]
export const GIVE_UP_MS = 6 * 60_000; // [guess]: covers BF-60's slowest measured placement (179 s) twice
// How long a start waits for the platform to refuse placement before it counts the container as started [guess]: the
// Containers class retries a refused start every 300 ms, so a refusal comes within one try [inference].
export const PLACED_WAIT_MS = 3_000;
// The container's inactivity timeout: the API's maximum, 6 h, which is the job limit (`JOB_LIMIT`). Without it the
// runtime stops the container shortly after the object goes idle (`crates/git/src/host.rs`, `set_inactivity`).
export const JOB_MS = 6 * 3_600_000;
// The runner is PID 1: `exec` replaces the shell, so the runner's exit stops the container.
export const ENTRYPOINT = ["/bin/sh", "-c", 'exec /home/runner/run.sh --jitconfig "$JIT"'];
const KEY = "runner";
const NO_INSTANCE = /no container instance/i;

// Stores the start once: a second arm for the same runner changes nothing.
export async function arm(o, instance, jit) {
  if (await o.ctx.storage.get(KEY)) return false;
  await o.ctx.storage.put(KEY, { state: "pending", instance, jit, armedAt: Date.now() });
  await o.ctx.storage.setAlarm(Date.now());
  return true;
}

// The object's alarm. Every error schedules the next try; if even that write fails the handler throws, and the
// runtime retries the alarm [doc: Durable Objects alarms, retries].
export async function alarm(o) {
  try {
    await step(o);
  } catch (e) {
    console.error(`graft-runners: start: ${e?.message ?? e}; again in ${RETRY_S} s`);
    await o.ctx.storage.setAlarm(Date.now() + RETRY_S * 1_000);
  }
}

async function step(o) {
  const st = await o.ctx.storage.get(KEY);
  if (st?.state !== "pending") return;
  const c = o.ctx.container;
  const image = c.images?.runner;
  if (Date.now() - st.armedAt > GIVE_UP_MS) {
    // One line per runner given up, with its image: a canary whose image never places shows here by version.
    console.error(`graft-runners: gave up after ${GIVE_UP_MS / 1_000} s, image ${image}; left to go stale`);
    return close(o);
  }
  // A retried alarm, or one after a crash between the start and the write below, finds the container and starts none.
  if (!c.running) {
    if (!image) throw new Error('no image "runner" in this version\'s image map');
    // A refused placement (no instance, or the account's vCPU limit) throws here or rejects the monitor with "no
    // container instance" [unverified which; the Containers class sees both, `container.js`, `isNoInstanceError`], and
    // either throw schedules the next try. Any other end within PLACED_WAIT_MS is the runner's own exit.
    c.start({ image, instance: st.instance, enableInternet: true, env: { JIT: st.jit }, entrypoint: ENTRYPOINT });
    const mon = c.monitor().then(
      () => "exited",
      (e) => (NO_INSTANCE.test(String(e?.message ?? e)) ? Promise.reject(e) : "exited"),
    );
    mon.catch(() => {});
    const how = await Promise.race([mon, o.sleep(PLACED_WAIT_MS).then(() => "up")]);
    if (how === "exited") {
      // The runner ended before it could take a job: a broken image. Its registration goes stale at HOSTED_IDLE.
      console.error(`graft-runners: runner exited within ${PLACED_WAIT_MS / 1_000} s of its start, image ${image}`);
    } else {
      console.log(`graft-runners: launched ${Math.round((Date.now() - st.armedAt) / 1_000)} s after arm, image ${image}`);
    }
  }
  // Also on a retried alarm that found the container: a crash after `start` must not leave it without the timeout.
  if (c.running) await c.setInactivityTimeout(JOB_MS);
  // A destroy that came while the start waited wins: the runner was deleted, so its container goes too.
  if ((await o.ctx.storage.get(KEY))?.state === "closed") return close(o);
  await o.ctx.storage.put(KEY, { state: "launched", image });
}

// The constructor's half of the inactivity timeout: an object that restarts while its container runs starts without
// one, so it sets it again (`crates/git/src/host.rs`, `set_inactivity`).
export async function wake(o) {
  if (o.ctx.container?.running) await o.ctx.container.setInactivityTimeout(JOB_MS);
}

// `graft-runners`' destroy. False, writing nothing, for a runner this object never held: an object named for a runner
// whose start never reached it has no container to stop.
export async function destroy(o) {
  if (!(await o.ctx.storage.get(KEY))) return false;
  await close(o);
  return true;
}

// A start given up, or destroyed: drops the start and its JIT config, then stops the container.
export async function close(o) {
  await o.ctx.storage.put(KEY, { state: "closed" });
  if (o.ctx.container.running) await o.ctx.container.destroy();
}
