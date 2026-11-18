// BF-54's teardown (tests/m4/wp4/bf54.mjs): in each Sandbox, SIGINT every listener, whose graceful exit deletes its
// session and, for an ephemeral runner, its registration (`delete_session`), then destroy the Sandbox. Prints what
// each step answered. The scratch Worker and its container application are deleted after, with wrangler.
//   BOXES=25 BOX_URL=... BOX_TOKEN_FILE=/tmp/bf54-box.token node tests/m4/wp4/bf54-teardown.mjs
import { readFileSync } from "node:fs";

const BOXES = Number(process.env.BOXES ?? 25);
const BOX = process.env.BOX_URL ?? `https://graft-runners-bf54.${process.env.CF_RUNNER_WORKERS_SUBDOMAIN}.workers.dev`;
const T = readFileSync(process.env.BOX_TOKEN_FILE ?? "/tmp/bf54-box.token", "utf8").trim();
const call = async (path, name, body = "") => {
  try {
    const r = await fetch(`${BOX}${path}?box=${name}`, { method: "POST", headers: { "x-t": T }, body, signal: AbortSignal.timeout(120_000) });
    return await r.json();
  } catch (e) { return { error: String(e).slice(0, 200) }; }
};
await Promise.all([...Array(BOXES).keys()].map(async (b) => {
  const name = `bf54-box-${b}`;
  const stop = await call("/exec", name, "pkill -INT -f Runner.Listener; for i in $(seq 30); do pgrep -f Runner.Listener >/dev/null || break; sleep 1; done; pgrep -fc Runner.Listener");
  const d = await call("/destroy", name);
  console.log(`${new Date().toISOString()} ${name}: listeners left ${(stop.stdout ?? stop.error ?? "").trim()}; destroy ${JSON.stringify(d)}`);
}));
