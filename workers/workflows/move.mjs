// M6-WP1's Workflows glue (design/DESIGN.md, "Install operations", Site loss; build/BUILD-PLAN.md, M6-WP1): the
// coordinator one `ReplaceMember` or `RestorePlacement` intent starts, the group instance it starts for each group, and
// the expiry instance each group starts for the member it removed. Plain JS, because workers-rs 0.8.7 has no Workflows
// bindings. Every call is a Durable Object binding to a site script's `Registry` or `Member` class (bound with
// `script_name` at install, spares included), so nothing here calls the Cloudflare API and no site script gets a new
// version. `entry.mjs` wraps these in `WorkflowEntrypoint`s; `tests/move.test.mjs` drives them with a fake `step` and
// fake bindings.
//
// The coordinator, from `graft-ops`' `MoveSpec` `{from, to, dead, home}`:
// 1. refills the spare's bucket from the surviving buckets, single-copy packs first (`/refill` on a surviving site's
//    registry object `refill`, `graftyard_git::refill_run`), a ReplaceMember only, since a restore's buckets are live;
// 2. lists every row of every registry shard on each surviving voter site (`/all_rows`), and keeps one per chain at
//    its highest generation (`graftyard_types::replace_candidates`): a row keeps the record its member joined under,
//    so a filter on rows naming `from` misses a group that moved there later;
// 3. starts one group instance per chain, in batches of 100, under ids derived from the intent and the chain, so a
//    coordinator step that ran twice starts nothing twice;
// 4. fills the rest of the spare's bucket;
// 5. waits until every group instance is complete, and errors if any errored, which `graft-ops` pages on.
//
// A group instance asks the chain's leader `/move` with `{"replace": {lost, spare}, started, dead}` until it answers
// `Done` (`graftyard_identity::group_move::replace_plan`): a chain whose committed config holds no voter on `from` is
// Done at once. A `notLeader` answer names the leader to ask next; a member that does not answer is passed over for
// the next voter its last config names.
//
// The expiry instance is `deleteAll()` after 35 days (`workers/site/src/replace.rs`): it sleeps `DELETE_AFTER` from the
// move, then asks the removed member `/expire`, naming the member that replaced it, once a day until it wipes.

// Workflow instance ids: `^[a-zA-Z0-9_][a-zA-Z0-9-_]*$`, at most 100 characters
// (https://developers.cloudflare.com/workflows/reference/limits/).
export const BATCH = 100;
// How long a group instance waits between asks while a step is held or the group is between leaders [guess].
export const ASK_EVERY = "5 seconds";
// How many asks a group instance makes before it errors: a group stuck an hour pages through `graft-ops` [guess].
export const ASKS_MAX = 720;
// How long the coordinator waits between looks at its group instances [guess].
export const POLL_EVERY = "30 seconds";
// The group instances one coordinator step looks at, at most: a step's subrequests stay bounded at any group count.
export const POLL_MAX = 200;
// The registry shards per site (`graftyard_types::REGISTRY_SHARDS`).
export const REGISTRY_SHARDS = 64;
// `workers/site/src/replace.rs`, `DELETE_AFTER`.
export const DELETE_AFTER = "35 days";
// How many daily asks an expiry instance makes after `DELETE_AFTER` before it gives up: a lost site recovered within a
// month still wipes its old members [guess].
export const EXPIRE_TRIES = 30;

const STEP_RETRY = { retries: { limit: 10, delay: "10 seconds", backoff: "exponential" }, timeout: "15 minutes" };

// The instance id for `chain` under the coordinator `intent`.
export function groupId(intent, chain) {
  const id = `${intent}-${String(chain).replace(/[^a-zA-Z0-9_-]/g, "_")}`;
  if (id.length > 97) throw new Error(`instance id too long: ${id}`);
  return id;
}

// `replace_candidates`: one row per chain, the highest generation.
export function candidates(rows) {
  const by = new Map();
  for (const r of rows) {
    const e = by.get(r.chain);
    if (!e || r.generation > e.generation) by.set(r.chain, r);
  }
  return [...by.values()].sort((a, b) => (a.chain < b.chain ? -1 : a.chain > b.chain ? 1 : 0));
}

// The voter sites a coordinator reads registries from: every non-spare site but `from`, the site the groups leave.
export function surviving(sites, spec) {
  return sites.filter((s) => !s.spare && s.id !== spec.from).map((s) => s.id);
}

// `env.REGISTRY_<site>` / `env.MEMBER_<site>`, by name with the site's location hint, as the Rust callers make them.
function stub(env, cls, site, name) {
  const s = env.sites.find((x) => x.id === site);
  const ns = env[`${cls}_${site}`];
  if (!ns || !s) throw new Error(`no ${cls}_${site} binding`);
  return ns.get(ns.idFromName(name), { locationHint: s.region });
}

async function json(res, what) {
  if (res.status !== 200) throw new Error(`${what}: HTTP ${res.status} ${await res.text()}`);
  return res.json();
}

// The coordinator's run (`MoveCoordinator`).
export async function coordinate(env, event, step) {
  const spec = event.payload;
  const intent = event.instanceId;
  const sites = surviving(env.sites, spec);
  if (sites.length === 0) throw new Error("no surviving site to enumerate");
  const refill = async (single) => {
    let from = null;
    for (let k = 0; ; k++) {
      const r = await step.do(`refill ${single ? "single" : "all"} ${k}`, STEP_RETRY, async () => {
        const res = await stub(env, "REGISTRY", sites[0], "refill").fetch("https://registry/refill", {
          method: "POST",
          body: JSON.stringify({ lost: spec.from, spare: spec.to, single, from }),
        });
        return json(res, "refill");
      });
      if (r.failed.length > 0) throw new Error(`refill failed: ${JSON.stringify(r.failed.slice(0, 5))}`);
      if (r.next === null) return;
      from = r.next;
    }
  };
  if (spec.dead) await refill(true);
  // One step per shard, so a retry reads one shard again and no step's result nears the 1 MiB cap. A row names its
  // own site's member, so each keeps the site it was read on.
  let rows = [];
  for (const site of sites) {
    for (let shard = 0; shard < REGISTRY_SHARDS; shard++) {
      const got = await step.do(`rows ${site}/${shard}`, STEP_RETRY, async () => {
        const res = await stub(env, "REGISTRY", site, `registry#${shard}`).fetch("https://registry/all_rows");
        return json(res, `rows ${site}/${shard}`);
      });
      rows = rows.concat(got.map((r) => ({ ...r, site })));
    }
  }
  const chains = candidates(rows);
  for (let i = 0; i < chains.length; i += BATCH) {
    await step.do(`start ${i}`, STEP_RETRY, async () => {
      const instances = chains.slice(i, i + BATCH).map((r) => ({
        id: groupId(intent, r.chain),
        params: { ...spec, chain: r.chain, row: r },
      }));
      const out = await env.GROUP.createBatch({ instances });
      // 10405: started by an earlier try of this step.
      const bad = out.errors.filter((e) => e.code !== 10405);
      if (bad.length > 0) throw new Error(`createBatch: ${JSON.stringify(bad.slice(0, 5))}`);
      return out.created.length;
    });
  }
  if (spec.dead) await refill(false);
  // Groups finish in any order, and a complete one stays complete: each step looks on from the first one not yet
  // seen complete, at most POLL_MAX of them.
  let next = 0;
  for (let k = 0; next < chains.length; k++) {
    const r = await step.do(`poll ${k}`, STEP_RETRY, async () => {
      let i = next;
      for (let n = 0; i < chains.length && n < POLL_MAX; n++, i++) {
        const s = await (await env.GROUP.get(groupId(intent, chains[i].chain))).status();
        if (s.status === "errored" || s.status === "terminated") {
          return { next: i, errored: [chains[i].chain, s.error?.message ?? s.status] };
        }
        if (s.status !== "complete") break;
      }
      return { next: i };
    });
    if (r.errored) throw new Error(`group ${r.errored[0]} failed: ${r.errored[1]}`);
    next = r.next;
    if (next < chains.length) await step.sleep(`wait ${k}`, POLL_EVERY);
  }
  return { groups: chains.length };
}

// A config as the site writes it: `[index, cfg]` (`/move`'s `config`), `{index, config}` (a `notLeader`'s), or bare.
function cfgOf(x) {
  if (!x) return null;
  if (Array.isArray(x)) return x[1];
  return x.config ?? x;
}

// The voters a `GroupConfig` JSON names (`{"normal": p}`, `{"degraded": {placement}}`, `{"joint": {old, new}}`).
export function configMembers(x) {
  const c = cfgOf(x);
  if (!c) return null;
  const halves = c.normal ? [c.normal] : c.degraded ? [c.degraded.placement] : c.joint ? [c.joint.old, c.joint.new] : [];
  const out = [];
  for (const p of halves) for (const v of p.voters ?? []) if (!out.some((m) => m.do_name === v.do_name)) out.push(v);
  return out.length > 0 ? out : null;
}

// One `/move` ask: the member answers `{"step", "ok"|"error", "config"}`; a follower answers `{"error":
// {"notLeader": {"leader"}}}`. Returns `{done}`, `{leader}` to ask next, `{campaign}`, `{held}`, or `{stepped}`.
// `lost` is the site a dead replace leaves: a follower still naming a leader there (its promise lapsed, but nothing
// asked it to campaign) or naming none is asked to campaign, since no other caller may reach an idle group.
export function judge(answer, members, lost = null) {
  const e = answer.error;
  if (e && typeof e === "object" && e.notLeader) {
    const l = e.notLeader.leader;
    if (lost !== null && (l === null || l === undefined || l === lost)) return { campaign: true };
    const next = members.find((m) => m.site === l);
    return next ? { leader: next } : { held: "no leader yet" };
  }
  if (answer.step === "Done") return { done: true };
  if (e) return { held: typeof e === "string" ? e : JSON.stringify(e) };
  if (answer.ok && answer.ok.warming) return { held: "warming" };
  return { stepped: answer.step };
}

// A group instance's run (`MoveGroup`).
export async function group(env, event, step) {
  const p = event.payload;
  let member = { site: p.row.site, do_name: p.row.do_name };
  let members = [member];
  let started = false;
  // The voter on `from` the first config names: the member this move removes, which the expiry instance wipes.
  let removed = null;
  for (let k = 0; k < ASKS_MAX; k++) {
    const r = await step.do(`ask ${k}`, STEP_RETRY, async () => {
      try {
        const res = await stub(env, "MEMBER", member.site, member.do_name).fetch("https://member/move", {
          method: "POST",
          body: JSON.stringify({ replace: { lost: p.from, spare: p.to }, started, dead: p.dead }),
        });
        if (res.status !== 200) return { error: `HTTP ${res.status}` };
        return await res.json();
      } catch (e) {
        // A member on a lost site, or a call cut off: asked no more this round.
        return { error: `call: ${e.message}` };
      }
    });
    members = configMembers(r.config ?? r.error?.notLeader?.config) ?? members;
    removed ??= members.find((m) => m.site === p.from) ?? null;
    const v = judge(r, members, p.dead ? p.from : null);
    if (v.done) {
      const by = members.find((m) => m.site === p.to);
      if (started && removed && by) {
        await step.do("expiry", STEP_RETRY, async () => {
          const out = await env.EXPIRE.createBatch({
            instances: [{ id: `${event.instanceId}-x`, params: { member: removed, by, since: Date.now() } }],
          });
          const bad = out.errors.filter((e) => e.code !== 10405);
          if (bad.length > 0) throw new Error(`expiry: ${JSON.stringify(bad)}`);
          return true;
        });
      }
      return { chain: p.chain, asks: k + 1, removed: removed?.do_name ?? null };
    }
    if (v.leader) {
      member = v.leader;
      continue;
    }
    // The member asked is a live voter with no live leader: it campaigns, as lookup's create asks a home member to
    // (`graftyard_lookup`, `/campaign`). It waits out any promise it holds (`Promised`), so the next ask may find it
    // still following; ask it again after ASK_EVERY.
    if (v.campaign) {
      await step.do(`campaign ${k}`, STEP_RETRY, async () => {
        try {
          const res = await stub(env, "MEMBER", member.site, member.do_name).fetch("https://member/campaign", {
            method: "POST",
            body: "{}",
          });
          return res.status === 200 ? (await res.json()).verdict ?? null : `HTTP ${res.status}`;
        } catch (e) {
          return `call: ${e.message}`;
        }
      });
      await step.sleep(`after ${k}`, ASK_EVERY);
      continue;
    }
    if (v.stepped) started = true;
    // Held with no step planned (no answer, or a member that knows no leader): ask the next voter the config names,
    // past any on a lost `from`. A held step came from the leader, which is asked again.
    if (v.held && !r.step) {
      const live = members.filter((m) => !(p.dead && m.site === p.from));
      const at = live.findIndex((m) => m.do_name === member.do_name);
      if (live.length > 0) member = live[(at + 1) % live.length];
    }
    await step.sleep(`after ${k}`, ASK_EVERY);
  }
  throw new Error(`${p.chain}: not done after ${ASKS_MAX} asks`);
}

// An expiry instance's run (`MoveExpire`): `{member, by, since}` from its group instance.
export async function expire(env, event, step) {
  const p = event.payload;
  await step.sleep("keep", DELETE_AFTER);
  for (let k = 0; k < EXPIRE_TRIES; k++) {
    const r = await step.do(`expire ${k}`, STEP_RETRY, async () => {
      try {
        const res = await stub(env, "MEMBER", p.member.site, p.member.do_name).fetch("https://member/expire", {
          method: "POST",
          body: JSON.stringify({ since: p.since, by: { site: p.by.site, do_name: p.by.do_name } }),
        });
        // 409: the member holds no group, as after a wipe or a purge.
        if (res.status === 409) return { wiped: true, already: true };
        if (res.status !== 200) return { wiped: false, error: `HTTP ${res.status}` };
        return await res.json();
      } catch (e) {
        return { wiped: false, error: `call: ${e.message}` };
      }
    });
    if (r.wiped) return { member: p.member.do_name, tries: k + 1 };
    await step.sleep(`day ${k}`, "1 day");
  }
  return { member: p.member.do_name, wiped: false };
}

// The handler `graft-ops` reaches over its `WORKFLOWS` service binding (`workers/ops/src/exec.rs`, `move_run` and
// `start_move`), the intent's ULID as the coordinator's id:
//   GET  /move/<intent>  {"status": <InstanceStatus.status>, "error"?}, or {"status": "absent"}
//   POST /move/<intent>  body `MoveSpec`; 200 once the coordinator exists, new or not
export async function handle(req, env) {
  const m = new URL(req.url).pathname.match(/^\/move\/([0-9A-Za-z]{26})$/);
  if (!m) return new Response("not found", { status: 404 });
  const id = m[1];
  if (req.method === "GET") {
    let s;
    try {
      s = await (await env.MOVE.get(id)).status();
    } catch {
      return Response.json({ status: "absent" });
    }
    return Response.json({ status: s.status, ...(s.error ? { error: s.error.message } : {}) });
  }
  if (req.method === "POST") {
    const spec = await req.json();
    try {
      await env.MOVE.create({ id, params: spec });
    } catch (e) {
      if (!String(e?.message).includes("(instance.already_exists)")) throw e;
    }
    return Response.json({ status: "started" });
  }
  return new Response("method not allowed", { status: 405 });
}

