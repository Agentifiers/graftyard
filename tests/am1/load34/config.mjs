// graft-m1-load's wrangler config (deploy.sh), and the test's (load34.test.mjs), which binds `MEMBER_<site>` to a fake
// member class in its own entry instead of the sites' scripts.
export function config({ name = "graft-m1-load", main = "../load34.mjs", members = ["graft-m1-site-a", "graft-m1-site-b", "graft-m1-site-c"], memberClass = "Member", bucket = "graft-m1-load",
  flags = ["nodejs_compat", "global_fetch_strictly_public"] } = {}) {
  return {
    name, main, compatibility_date: "2026-10-01",
    // nodejs_compat: wire.mjs's Buffer, node:crypto and node:zlib (the pack builder's deflate level 1, byte-identical to a1's).
    // global_fetch_strictly_public: the edge is reached through the front door, as a client reaches it.
    compatibility_flags: flags,
    workers_dev: true, preview_urls: false,
    // A client runs 15 minutes and more from one alarm: its subrequests and CPU are one invocation's.
    limits: { cpu_ms: 300_000, subrequests: 10_000_000 },
    durable_objects: { bindings: [
      { name: "LOAD", class_name: "Load34" },
      ...members.map((script, site) => ({ name: `MEMBER_${site}`, class_name: memberClass, ...(script ? { script_name: script } : {}) })),
    ] },
    r2_buckets: [{ binding: "BUCKET", bucket_name: bucket }],
    migrations: [{ tag: "v1", new_sqlite_classes: ["Load34"] }],
    observability: { enabled: true },
  };
}

if (import.meta.url === `file://${process.argv[1]}`) console.log(JSON.stringify(config(), null, 2));
