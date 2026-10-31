// Prototype site script. One source, deployed three times as graft-site-a, graft-site-b, and graft-site-c, each
// defining its own member classes and binding the other two sites' classes with `script_name` (design/DESIGN.md,
// Shape, "Members reach each other through Durable Object bindings"). M0-WP1 only: no Raft yet.

import { DurableObject } from "cloudflare:workers";
import { median, SITES, type MemberClass, type MemberInfo, type MemberRef, type MemberReport, type RttRow, type SiteKey } from "./sites.ts";

type Ns = DurableObjectNamespace<Member>;
export interface SiteEnv {
  SITE: SiteKey;
  /** The `Rtt` recheck period. The design's is a day; B0-15 runs it hourly (decisions/DECISIONS.md, "Milestone 0"). */
  RTT_RECHECK_MS: string;
  MEMBER_A: Ns; MEMBER_B: Ns; MEMBER_C: Ns;
  IDLE_A: Ns; IDLE_B: Ns; IDLE_C: Ns;
  CONTROL_A: Ns; CONTROL_B: Ns; CONTROL_C: Ns;
}
export type BindingsEnv = Omit<SiteEnv, "SITE" | "RTT_RECHECK_MS">;

export function memberStub(env: BindingsEnv, ref: MemberRef): DurableObjectStub<Member> {
  const ns = env[`${(ref.cls ?? "member").toUpperCase()}_${ref.site.toUpperCase()}` as keyof BindingsEnv];
  if (!ns) throw new Error(`${ref.cls ?? "member"} on site ${ref.site} is not bound in this script`);
  // A location hint is respected only by the first get() of an object, so every caller passes the site's hint.
  return ns.get(ns.idFromName(ref.name), { locationHint: SITES[ref.site].hint });
}

const RTT_SAMPLES = 20;
const HOUR_MS = 3_600_000;

export class Member extends DurableObject<SiteEnv> {
  protected sql = this.ctx.storage.sql;

  constructor(ctx: DurableObjectState, env: SiteEnv) {
    super(ctx, env);
    this.sql.exec(`CREATE TABLE IF NOT EXISTS meta (k TEXT PRIMARY KEY, v TEXT NOT NULL)`);
    // Shape's `Rtt`: one row per other voter, in the member's own SQLite, outside any group config.
    this.sql.exec(`CREATE TABLE IF NOT EXISTS rtt (peer TEXT PRIMARY KEY, peer_site TEXT NOT NULL,
                   median_ms REAL NOT NULL, samples TEXT NOT NULL, at INTEGER NOT NULL)`);
  }

  protected meta(k: string): string | undefined {
    return this.sql.exec<{ v: string }>(`SELECT v FROM meta WHERE k = ?`, k).toArray()[0]?.v;
  }
  protected setMeta(k: string, v: string | number): void {
    this.sql.exec(`INSERT OR REPLACE INTO meta VALUES (?, ?)`, k, String(v));
  }

  /** Creates the member on first call and reports the colo it runs in. */
  async info(ref: MemberRef): Promise<MemberInfo> {
    let colo = this.meta("colo");
    const fresh = colo === undefined;
    if (fresh) {
      const trace = await (await fetch("https://www.cloudflare.com/cdn-cgi/trace")).text();
      colo = /^colo=(\w+)$/m.exec(trace)?.[1] ?? "unknown";
      this.setMeta("colo", colo);
      this.setMeta("name", ref.name);
      this.setMeta("site", ref.site);
      this.setMeta("created", Date.now());
    }
    return { ...ref, colo: colo!, createdAt: Number(this.meta("created")), fresh };
  }

  ping(): number {
    return Date.now();
  }

  /** Median of RTT_SAMPLES sequential binding round trips to each peer, stored as `Rtt` rows. */
  async measure(self: MemberRef, peers: MemberRef[]): Promise<RttRow[]> {
    // Peers are measured side by side; the samples to one peer stay sequential.
    const rows = await Promise.all(peers.map(async (peer): Promise<RttRow> => {
      const stub = memberStub(this.env, peer);
      const samples: number[] = [];
      for (let i = 0; i < RTT_SAMPLES; i++) {
        const t0 = Date.now(); // advances only across I/O in Workers, which is what each sample is
        await stub.ping();
        samples.push(Date.now() - t0);
      }
      return { from: self.name, peer: peer.name, peerSite: peer.site, medianMs: median(samples), samples, at: Date.now() };
    }));
    for (const row of rows) {
      this.sql.exec(`INSERT OR REPLACE INTO rtt VALUES (?, ?, ?, ?, ?)`,
        row.peer, row.peerSite, row.medianMs, JSON.stringify(row.samples), row.at);
    }
    this.setMeta("rtt_checks", Number(this.meta("rtt_checks") ?? 0) + 1);
    this.setMeta("last_measured", Date.now());
    return rows;
  }

  /** A member joins: it creates itself, keeps its peers, and measures them once (Shape, "Sites and placement"). */
  async join(self: MemberRef, peers: MemberRef[]): Promise<{ info: MemberInfo; rtt: RttRow[] }> {
    const info = await this.info(self);
    this.setMeta("self", JSON.stringify(self));
    this.setMeta("peers", JSON.stringify(peers));
    this.setMeta("joined", Date.now());
    return { info, rtt: await this.measure(self, peers) };
  }

  /** Serving a request is the only thing that rechecks `Rtt`, and only in a period it has not measured yet, so a
   *  member that serves nothing never wakes to measure. */
  async serve(): Promise<void> {
    this.setMeta("served", Number(this.meta("served") ?? 0) + 1);
    const period = Number(this.env.RTT_RECHECK_MS);
    const last = Number(this.meta("last_measured") ?? 0);
    if (Math.floor(Date.now() / period) > Math.floor(last / period)) {
      await this.measure(JSON.parse(this.meta("self")!), JSON.parse(this.meta("peers")!));
    }
  }

  /** Read-only. Does not count as serving, so reading the result cannot cause a recheck. */
  report(): MemberReport {
    return {
      name: this.meta("name") ?? "", served: Number(this.meta("served") ?? 0),
      rttChecks: Number(this.meta("rtt_checks") ?? 0), lastMeasuredAt: Number(this.meta("last_measured") ?? 0),
      joinedAt: Number(this.meta("joined") ?? 0),
    };
  }
}

/** B0-15's idle groups: the same code, in a namespace of their own. Nothing calls them after `join`. */
export class IdleMember extends Member {}

/** B0-15's control groups: each member serves one request an hour, driven by its own alarm so the run needs no
 *  outside clock. The alarm stands in for a client request; it runs the same `serve` path. */
export class ControlMember extends Member {
  async startHourly(firstInMs: number): Promise<void> {
    await this.ctx.storage.setAlarm(Date.now() + firstInMs);
  }
  async alarm(): Promise<void> {
    await this.serve();
    if (this.meta("stopped") !== "1") await this.ctx.storage.setAlarm(Date.now() + HOUR_MS);
  }
  async stop(): Promise<void> {
    this.setMeta("stopped", "1");
    await this.ctx.storage.deleteAlarm();
  }
}

export default {
  // Site scripts have no public route in the design. workers.dev is disabled in wrangler config.
  async fetch(): Promise<Response> {
    return new Response("not found", { status: 404 });
  },
} satisfies ExportedHandler<SiteEnv>;
