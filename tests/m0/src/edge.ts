// Prototype edge for Milestone 0: the only public entry, bearer-token guarded. It binds every site's member classes
// with `script_name`, as the design's edge does, and drives members for the runners.

import { memberStub, type BindingsEnv } from "./site.ts";
import type { MemberRef } from "./sites.ts";

interface EdgeEnv extends BindingsEnv { M0_TOKEN: string }

const json = (v: unknown, status = 200) =>
  new Response(JSON.stringify(v), { status, headers: { "content-type": "application/json" } });

async function settle<T>(xs: Promise<T>[]): Promise<({ ok: true; value: T } | { ok: false; error: string })[]> {
  return (await Promise.allSettled(xs)).map((r) =>
    r.status === "fulfilled" ? { ok: true, value: r.value } : { ok: false, error: String(r.reason) });
}

type Job = { member: MemberRef; peers: MemberRef[] };

export default {
  async fetch(req, env): Promise<Response> {
    if (req.headers.get("authorization") !== `Bearer ${env.M0_TOKEN}`) return json({ error: "unauthorized" }, 401);
    const url = new URL(req.url);
    const colo = (req.cf as { colo?: string } | undefined)?.colo;
    const body = req.method === "POST" ? await req.json<Record<string, any>>() : {};
    const members = (body.members ?? []) as MemberRef[];
    const jobs = (body.jobs ?? []) as Job[];
    // Batches keep each invocation under the subrequest limit.
    switch (url.pathname) {
      case "/colo": return json({ colo });
      case "/create": return json({ colo, results: await settle(members.map((m) => memberStub(env, m).info(m))) });
      case "/measure": return json({ colo, results: await settle(jobs.map((j) => memberStub(env, j.member).measure(j.member, j.peers))) });
      case "/join": return json({ colo, results: await settle(jobs.map((j) => memberStub(env, j.member).join(j.member, j.peers))) });
      case "/report": return json({ colo, results: await settle(members.map((m) => memberStub(env, m).report())) });
      case "/control/start": {
        const offsets = body.offsetsMs as number[];
        return json({ colo, results: await settle(members.map((m, i) =>
          (memberStub(env, m) as unknown as DurableObjectStub<import("./site.ts").ControlMember>).startHourly(offsets[i]))) });
      }
      case "/control/stop": return json({ colo, results: await settle(members.map((m) =>
        (memberStub(env, m) as unknown as DurableObjectStub<import("./site.ts").ControlMember>).stop())) });
    }
    return json({ error: "not found" }, 404);
  },
} satisfies ExportedHandler<EdgeEnv>;
