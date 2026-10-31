// M0-WP8 edge, graft-wp8-edge: the only public entry, behind the harness bearer token. It binds the three member
// classes and the three site buckets. A push names real git objects (made by `git` on the runner and posted as type and
// raw body); the edge checks each object's SHA-1, parses its links, and either inlines the objects (at most INLINE_MAX)
// or places them as one stored pack with create-only puts in the three buckets (durable at two), then commits the push
// through the leader. `/audit` checks every member's truncations against the committed `Archived` records and HEADs every
// committed key in its two buckets. `/read` walks a ref's closure from R2 and checks every object's SHA-1.

import { WorkerEntrypoint } from "cloudflare:workers";
import type { MemberEnv, NotLeader, SubmitRes, Wp8Member } from "./member.ts";
import { closure, headIn, linksOf, loadPacks, objectOk, packId, placePack } from "./store.ts";
import { HINT, INLINE_MAX, SITES, memberName, type ArchivedRec, type GitObj, type ObjType, type PackRef, type RefUpdate, type SiteKey } from "./types.ts";

interface EdgeEnv extends MemberEnv { M0_TOKEN: string }
const json = (v: unknown, status = 200) => Response.json(v, { status });

export class Wp8Edge extends WorkerEntrypoint<EdgeEnv> {
  private member(group: string, s: SiteKey): DurableObjectStub<Wp8Member> {
    const ns = this.env[`MEMBER_${s.toUpperCase()}` as "MEMBER_A"];
    return ns.get(ns.idFromName(memberName(group, s)), { locationHint: HINT[s] });
  }
  private async views(group: string) { return Promise.all(SITES.map((s) => this.member(group, s).view(group))); }
  private async leader(group: string): Promise<SiteKey> {
    const v = await this.views(group);
    const l = v.filter((x) => x.status.role === "leader").sort((a, b) => b.status.term - a.status.term)[0];
    if (!l) throw new Error("no leader");
    return l.status.site;
  }
  /** Sends to the leader, following one `NotLeader`. */
  private async toLeader<T extends SubmitRes>(group: string, f: (m: DurableObjectStub<Wp8Member>) => Promise<T>): Promise<T> {
    let s = await this.leader(group);
    for (let i = 0; i < 3; i++) {
      const r = (await f(this.member(group, s))) as T | NotLeader;
      if (!("notLeader" in r)) return r as T;
      if (!r.notLeader.leader) break;
      s = r.notLeader.leader;
    }
    throw new Error("no leader answered");
  }

  async fetch(req: Request): Promise<Response> {
    if (req.headers.get("authorization") !== `Bearer ${this.env.M0_TOKEN}`) return new Response("unauthorized", { status: 401 });
    const url = new URL(req.url);
    const body = req.method === "POST" ? await req.json<Record<string, any>>() : {};
    const group = String(body.group ?? "g0");
    try {
      switch (url.pathname) {
        case "/campaign": return json({ result: await this.member(group, body.site).campaign(group) });
        case "/clock": await Promise.all(SITES.map((s) => this.member(group, s).setClock(group, body.cal))); return json({ ok: true });
        case "/flags": return json(await this.member(group, body.site).setFlags(group, body.flags ?? {}));
        case "/tick": {
          // Leader first (archive, GC, sweep), then followers (snapshot, truncate).
          if (body.site) return json({ [body.site]: await this.member(group, body.site).tick(group) });
          const order = (await this.views(group)).sort((a, b) => Number(b.status.role === "leader") - Number(a.status.role === "leader")).map((v) => v.status.site);
          const out: Record<string, string[] | string> = {};
          for (const s of order) out[s] = await this.member(group, s).tick(group).catch((e) => `failed: ${String(e).slice(0, 120)}`);
          return json(out);
        }
        case "/push": return json(await this.push(group, body.rid, body.updates, body.objects ?? []));
        case "/indexAt": return json(await this.member(group, await this.leader(group)).indexAt(group, body.t));
        case "/restore": return json(await this.toLeader(group, (m) => m.restore(group, body.rid, body.to, body.refs, body.holdMs ?? 0)));
        // G6 race routes, all on the leader.
        case "/gcRace": return json(await this.member(group, await this.leader(group)).gcRace(group, body.holdMs ?? 0));
        case "/pushExisting": return json(await this.toLeader(group, (m) => m.pushExisting(group, body.rid, body.updates, body.holdMs ?? 0)));
        case "/gcPlan": return json(await this.member(group, await this.leader(group)).gcPlan(group));
        case "/lost": return json(await this.member(group, await this.leader(group)).lostCheck(group, body.oids ?? []));
        case "/refs": return json(await Promise.all(SITES.map((s) => this.member(group, s).refs(group).then((r) => ({ site: s, ...r }), (e) => ({ site: s, error: String(e) })))));
        case "/view": return json(await this.views(group));
        case "/audit": return json(await this.audit(group));
        case "/read": return json(await this.read(group, body.refs));
      }
    } catch (e) {
      return json({ error: String(e) }, 500);
    }
    return json({ error: "not found" }, 404);
  }

  private async push(group: string, rid: string, updates: RefUpdate[], raw: { oid: string; type: ObjType; data: string }[]) {
    const objects: GitObj[] = [];
    for (const o of raw) {
      const g: GitObj = { ...o, links: linksOf(o.type, o.data) };
      if (!(await objectOk(g))) return { out: { ok: false, why: `bad object ${o.oid}` }, entry: 0 };
      objects.push(g);
    }
    const leader = await this.leader(group);
    const cal = (await this.member(group, leader).view(group)).status.cal;
    const bytes = objects.reduce((n, o) => n + o.data.length, 0);
    let pack: PackRef | null = null;
    if (objects.length) pack = bytes <= INLINE_MAX ? { inline: objects } : { stored: await placePack(this.env, group, { id: packId(cal), cruft: false, objects }, rid) };
    return this.toLeader(group, (m) => m.submit(group, rid, { kind: "push", updates, pack }));
  }

  /** Every object reachable from each member's refs (or the given ref values), read from the live packs in R2. */
  private async read(group: string, refs?: Record<string, string>) {
    const v = await this.views(group);
    const l = v.find((x) => x.status.role === "leader") ?? v[0];
    const roots = refs ?? (await this.member(group, l.status.site).refs(group)).refs;
    const { objs, unreadable, reads } = await loadPacks(this.env, group, l.packs);
    const { reached, missing } = closure(objs, Object.values(roots));
    let bad = 0;
    for (const o of reached) if (!(await objectOk(o))) bad++;
    const byType: Record<string, number> = {};
    for (const o of reached) byType[o.type] = (byType[o.type] ?? 0) + 1;
    return { roots, packsRead: reads, unreadable, reached: reached.length, byType, missing, badSha1: bad };
  }

  /** The fail condition, checked from the outside: each truncation (or install) moved a member's base only through
   *  ranges covered, with no gap, by archive records committed in the leader's fold (or a committed snapshot plus
   *  archives), and every committed key is present with its digest in both of its buckets. */
  private async audit(group: string) {
    const v = await this.views(group);
    const l = v.filter((x) => x.status.role === "leader").sort((a, b) => b.status.term - a.status.term)[0] ?? v[0];
    const recs = l.archived;
    const archives = recs.filter((r) => r.kind === "archive").sort((a, b) => a.from - b.from);
    const covered = (from: number, to: number) => {
      let t = from - 1;
      for (const r of archives) if (r.from <= t + 1 && r.to > t) t = r.to;
      return t >= to;
    };
    const bad: string[] = [];
    for (const m of v) for (const t of m.trunc) {
      const ok = t.kind === "install"
        ? recs.some((r) => r.kind === "snapshot" && r.to <= t.after && covered(r.to + 1, t.after)) || covered(1, t.after)
        : covered(t.before + 1, t.after);
      if (!ok) bad.push(`${m.status.site} ${t.kind} ${t.before}->${t.after} at ${new Date(t.cal).toISOString().slice(0, 10)} has no committed Archived`);
    }
    const heads = await Promise.all(recs.map(async (r: ArchivedRec) => ({ key: r.key, kind: r.kind, sites: r.sites, head: await headIn(this.env, r.sites, r.key, r.digest) })));
    const notInTwo = heads.filter((h) => h.sites.length < 2 || !h.sites.every((s) => h.head[s]));
    return {
      leader: l.status.site, archives: archives.length, snapshots: recs.length - archives.length, archThrough: l.status.archThrough,
      truncations: v.map((m) => ({ site: m.status.site, base: m.status.base, rows: m.trunc.length, refused: m.status.refusedTruncations, installs: m.status.installs })),
      badTruncations: bad, committedKeysNotInTwoBuckets: notInTwo,
      foldDigests: Object.fromEntries(v.map((m) => [m.status.site, `${m.status.applied}:${m.status.foldDigest.slice(0, 12)}`])),
    };
  }
}

export default Wp8Edge;
