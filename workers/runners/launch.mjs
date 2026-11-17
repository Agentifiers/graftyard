// A hosted runner's start, run by its own Sandbox (design/DESIGN.md, Shape, `graft-runners`; decisions/DECISIONS.md,
// `f80-alarm-start`). A pass registers the runner and calls `arm`, which stores the start and schedules it on the
// Sandbox's alarm, then returns: no Worker invocation waits on placement. `launch` runs from that alarm (the Containers
// base class runs due `schedule` rows inside `alarm()`, 15 minutes of wall time, at least once). It starts the process
// unless the container already holds it, retries every RETRY_S while placement fails, and begins no try after
// GIVE_UP_MS. One try waits at most the SDK's transport budget (150 s), so the last start lands before the install's
// HOSTED_IDLE (600 s): a runner whose process never starts goes stale and is deleted, and never starts after that. The
// stored start, which carries the runner's JIT config, is dropped once it launches or closes.
// `sb` is the Sandbox object (`entry.mjs`): `ctx.storage`, `schedule`, `getProcess`, `startProcess`, `destroy`.

export const RETRY_S = 10; // [guess]
export const GIVE_UP_MS = 6 * 60_000; // [guess]: covers BF-60's slowest measured placement (179 s) twice
const KEY = "runner";

// Stores the start once: a second arm for the same Sandbox changes nothing.
export async function arm(sb, cmd, opts) {
  if (await sb.ctx.storage.get(KEY)) return false;
  await sb.ctx.storage.put(KEY, { state: "pending", cmd, opts, armedAt: Date.now() });
  await sb.schedule(0, "launchRunner");
  return true;
}

export async function launch(sb) {
  const st = await sb.ctx.storage.get(KEY);
  if (st?.state !== "pending") return;
  if (Date.now() - st.armedAt > GIVE_UP_MS) {
    console.error(`graft-runners: ${st.opts.processId} not started in ${GIVE_UP_MS / 1000} s; left to go stale`);
    return close(sb);
  }
  try {
    // A retried alarm, or one after a crash between the start and the write below, finds the process and starts none.
    if (!(await sb.getProcess(st.opts.processId))) await sb.startProcess(st.cmd, st.opts);
  } catch (e) {
    console.error(`graft-runners: start ${st.opts.processId}: ${e?.message ?? e}; again in ${RETRY_S} s`);
    await sb.schedule(RETRY_S, "launchRunner");
    return;
  }
  // A destroy that came while the start waited wins: the runner was deleted, so its process goes too.
  if ((await sb.ctx.storage.get(KEY))?.state === "closed") return sb.destroy();
  await sb.ctx.storage.put(KEY, { state: "launched" });
}

export async function close(sb) {
  await sb.ctx.storage.put(KEY, { state: "closed" });
}
