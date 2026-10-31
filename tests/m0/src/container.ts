// Container probe for M0 under the `durable_object` scheduling policy, the only policy this account may create
// (decisions/OPEN-QUESTIONS.md, C19). Its own script, so it never creates a site-script version.
//
// B0-16: a deploy by an account-owned token with only Workers Scripts Edit and Containers Edit (results/b0-16.md).
// Placement: under `durable_object`, Wrangler rejects `constraints.regions`, so a shard cannot pin its container to its
// site's region. Each call starts one container from a Durable Object created with a location hint, reads where the
// container landed from the runtime's own variables, and destroys it.

import { DurableObject } from "cloudflare:workers";

interface Env { GITBOX: DurableObjectNamespace<GitBox>; M0_TOKEN: string }

export interface Landing {
  doColo: string; started: boolean; startMs: number; region?: string; location?: string; country?: string;
  gitVersion?: string; error?: string;
}

export class GitBox extends DurableObject<Env> {
  async land(): Promise<Landing> {
    const trace = await (await fetch("https://www.cloudflare.com/cdn-cgi/trace")).text();
    const doColo = /^colo=(\w+)$/m.exec(trace)?.[1] ?? "unknown";
    const c = this.ctx.container!;
    const t0 = Date.now();
    try {
      if (!c.running) c.start({ image: (c as unknown as { images: Record<string, string> }).images.git, instance: "lite", enableInternet: false });
      // start() returns before the container is up; exec waits for a container that is still starting.
      for (let i = 0; i < 60; i++) {
        try {
          // exec() passes only PATH to its process, so read the runtime's variables from the main process (pid 1).
          const out = await (await c.exec(["sh", "-c", "tr '\\0' '\\n' < /proc/1/environ; git --version"])).output();
          const text = new TextDecoder().decode(out.stdout);
          const v = (k: string) => new RegExp(`^${k}=(.*)$`, "m").exec(text)?.[1];
          return { doColo, started: true, startMs: Date.now() - t0, region: v("CLOUDFLARE_REGION"), location: v("CLOUDFLARE_LOCATION"),
                   country: v("CLOUDFLARE_COUNTRY_A2"), gitVersion: /^git version (.*)$/m.exec(text)?.[1] };
        } catch {
          await new Promise((r) => setTimeout(r, 1000));
        }
      }
      return { doColo, started: false, startMs: Date.now() - t0, error: "no exec within 60 s" };
    } catch (e) {
      return { doColo, started: false, startMs: Date.now() - t0, error: String(e) };
    } finally {
      if (c.running) await c.destroy().catch(() => {});
    }
  }

  // Survival across a new Worker version: `keep` starts a container with a 15-minute inactivity timeout and reports
  // its boot id; after a deploy, `peek` reports whether the same container still runs, without starting one.
  private get c() { return this.ctx.container as unknown as {
    running: boolean; images: Record<string, string>; start(o: object): void; destroy(): Promise<void>;
    exec(cmd: string[]): Promise<{ output(): Promise<{ stdout: ArrayBuffer }> }>;
    inspect(): Promise<{ image: string } | null>; setInactivityTimeout(ms: number): Promise<void> }; }

  private async bootId(): Promise<string> {
    for (let i = 0; i < 60; i++) {
      try { return new TextDecoder().decode((await (await this.c.exec(["cat", "/proc/sys/kernel/random/boot_id"])).output()).stdout).trim(); }
      catch { await new Promise((r) => setTimeout(r, 1000)); }
    }
    return "none";
  }

  async keep(): Promise<object> {
    if (!this.c.running) this.c.start({ image: this.c.images.git, instance: "lite", enableInternet: false });
    await this.c.setInactivityTimeout(15 * 60_000);
    return { running: this.c.running, bootId: await this.bootId(), image: (await this.c.inspect())?.image, configured: this.c.images.git };
  }

  async peek(): Promise<object> {
    const running = this.c.running;
    if (running) await this.c.setInactivityTimeout(15 * 60_000); // a restarted Durable Object starts without one
    return { running, bootId: running ? await this.bootId() : null, image: running ? (await this.c.inspect())?.image : null,
             configured: this.c.images.git };
  }

  async stop(): Promise<void> {
    if (this.c.running) await this.c.destroy();
  }
}

export default {
  async fetch(req, env): Promise<Response> {
    if (req.headers.get("authorization") !== `Bearer ${env.M0_TOKEN}`) return new Response("unauthorized", { status: 401 });
    const url = new URL(req.url);
    const hint = (url.searchParams.get("hint") ?? undefined) as DurableObjectLocationHint | undefined;
    const name = url.searchParams.get("name") ?? "b0-16";
    const stub = env.GITBOX.get(env.GITBOX.idFromName(name), hint ? { locationHint: hint } : undefined);
    switch (url.searchParams.get("op")) {
      case "keep": return Response.json(await stub.keep());
      case "peek": return Response.json(await stub.peek());
      case "stop": await stub.stop(); return Response.json({ stopped: true });
    }
    return Response.json(await stub.land());
  },
} satisfies ExportedHandler<Env>;
