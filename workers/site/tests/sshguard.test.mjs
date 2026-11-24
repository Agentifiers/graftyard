// The SSH-off guard (`workers/site/sshguard.mjs`) over a stub Cloudflare API. Run with
// `node --test 'workers/site/tests/*.test.mjs'`.
import { test } from "node:test";
import assert from "node:assert/strict";
import { refuseForeignHostKey, refuseSshOff } from "../sshguard.mjs";

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

// One account and one store, two installs: graft-m3f's sites bind SSH_HOST_KEY_A and _B, graft-m4a's bind neither, and
// an unrelated script binds a same-named secret of another store.
function store(bindings, { fail = null } = {}) {
  return async (path) => {
    if (fail && path.includes(fail)) throw new Error("403 Authentication error");
    if (path.endsWith("/workers/scripts")) return { result: Object.keys(bindings).map((id) => ({ id })) };
    const s = path.match(/workers\/scripts\/([^/]+)\/settings$/);
    if (s) return { result: { bindings: bindings[s[1]] } };
    throw new Error(`unexpected ${path}`);
  };
}
const hk = (slot, store_id = "st1") => ({ type: "secrets_store_secret", name: `SSH_HOST_KEY_${slot}`, store_id, secret_name: `SSH_HOST_KEY_${slot}` });
const ACCOUNT = {
  "graft-m3f-site-a": [hk("A"), hk("B")], "graft-m3f-site-b": [hk("A"), hk("B")], "graft-m3f-site-c": [hk("A"), hk("B")],
  "graft-m4a-site-a": [{ type: "plain_text", name: "SITE_ID" }],
  "other-store-user": [hk("B", "st2")],
};
const M4A = ["graft-m4a-site-a", "graft-m4a-site-b", "graft-m4a-site-c"];

test("a host-key write is refused when another install's scripts bind the secret", async () => {
  await assert.rejects(refuseForeignHostKey(store(ACCOUNT), "acct", { store: "st1", secret: "SSH_HOST_KEY_B", siteScripts: M4A }),
    /SSH_HOST_KEY_B is bound by another install's scripts \(graft-m3f-site-a, graft-m3f-site-b, graft-m3f-site-c\)/);
});

test("a host-key write goes ahead for the install that binds it, or when nothing binds it in this store", async () => {
  await refuseForeignHostKey(store(ACCOUNT), "acct", { store: "st1", secret: "SSH_HOST_KEY_B", siteScripts: SITES });
  // other-store-user binds SSH_HOST_KEY_B of st2, not st1's.
  const { "graft-m3f-site-a": _a, "graft-m3f-site-b": _b, "graft-m3f-site-c": _c, ...rest } = ACCOUNT;
  await refuseForeignHostKey(store(rest), "acct", { store: "st1", secret: "SSH_HOST_KEY_B", siteScripts: M4A });
});

test("a host-key write is refused when the bindings cannot be listed", async () => {
  await assert.rejects(refuseForeignHostKey(store(ACCOUNT, { fail: "graft-m3f-site-b" }), "acct",
    { store: "st1", secret: "SSH_HOST_KEY_A", siteScripts: SITES }), /cannot list which scripts bind SSH_HOST_KEY_A/);
});
