// The SSH-off guard (`workers/site/sshguard.mjs`) over a stub Cloudflare API. Run with
// `node --test 'workers/site/tests/*.test.mjs'`.
import { test } from "node:test";
import assert from "node:assert/strict";
import { refuseSshOff } from "../sshguard.mjs";

const SITES = ["graft-m3f-site-a", "graft-m3f-site-b", "graft-m3f-site-c"];

// An account with zones `z1` (apps in `apps.z1`) and `z2`, the scripts below, and each script's bindings.
function api(apps, { fail = null } = {}) {
  const scripts = [
    { id: "graft-m3f-edge", tag: "t-m3f" },
    { id: "graft-m4a-edge", tag: "t-m4a" },
    { id: "front-of-m3f", tag: "t-front" },
  ];
  const bindings = {
    "graft-m4a-edge": [{ type: "durable_object_namespace", class_name: "SshFront", script_name: "graft-m4a-site-a" }],
    "front-of-m3f": [{ type: "durable_object_namespace", class_name: "SshFront", script_name: "graft-m3f-site-b" }],
  };
  return async (path) => {
    if (fail && path.startsWith(fail)) throw new Error("403 Authentication error");
    if (path.startsWith("zones?")) return { result: [{ id: "z1" }, { id: "z2" }] };
    const m = path.match(/^zones\/(\w+)\/spectrum\/apps/);
    if (m) return { result: apps[m[1]] ?? [] };
    if (path.endsWith("/workers/scripts")) return { result: scripts };
    const s = path.match(/workers\/scripts\/([^/]+)\/settings$/);
    if (s) return { result: { bindings: bindings[s[1]] ?? [] } };
    throw new Error(`unexpected ${path}`);
  };
}
const app = (name, tag, traffic = "worker") => ({ dns: { name }, protocol: "tcp/22", traffic_type: traffic, origin_worker_id: tag });

test("a deploy without --ssh is refused while a Spectrum app's Worker origin is this install's edge", async () => {
  const a = api({ z2: [app("ssh.graftyard.dev", "t-m3f")] });
  await assert.rejects(refuseSshOff(["--test-faults"], { edge: "graft-m3f-edge", siteScripts: SITES }, a), /turns native SSH off for ssh\.graftyard\.dev/);
  // A site deploy does not know the edge's name, so it finds the origin by its SshFront bindings on our sites.
  const front = api({ z1: [app("ssh.graftyard.dev", "t-front")] });
  await assert.rejects(refuseSshOff([], { siteScripts: SITES }, front), /ssh\.graftyard\.dev \(origin front-of-m3f\)/);
});

test("--ssh and --ssh-off deploy without asking, and other installs' or non-Worker apps do not refuse", async () => {
  const a = api({ z1: [app("ssh.graftyard.dev", "t-m3f")] }, { fail: "zones" });
  await refuseSshOff(["--ssh", "k.json"], { edge: "graft-m3f-edge", siteScripts: SITES }, a);
  await refuseSshOff(["--ssh-off"], { edge: "graft-m3f-edge", siteScripts: SITES }, a);
  const others = api({ z1: [app("ssh.m4a.example", "t-m4a"), app("tcp.example", "t-m3f", "direct")], z2: [] });
  await refuseSshOff([], { edge: "graft-m3f-edge", siteScripts: SITES }, others);
  await refuseSshOff([], { siteScripts: SITES }, others);
});

test("an API failure refuses, since the guard cannot tell", async () => {
  await assert.rejects(refuseSshOff([], { siteScripts: SITES }, api({}, { fail: "zones" })), /cannot list Spectrum apps/);
});
