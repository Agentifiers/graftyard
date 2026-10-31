// M0-WP16 L0-10: the vault, script `graft-wp16-vault` with its own bucket `graft-wp16-vault` (design/DESIGN.md,
// Install operations, Vault and Whole-install restore). It shares no binding with the install: it reaches the install's
// edge (src/wp16/prov.ts) only over HTTPS, pulling with the export token, which reads `/export/*` and nothing else.
//
//   POST /pull     {max}      pulls up to `max` entries after its cursor, then every pack those entries name (checked
//                             against the entry's `checksum`) and every new snapshot; stores them as `log/mono/<from>-<to>`,
//                             `pack/mono/<id>`, `snapshot/mono/...`. Idempotent: the cursor moves only after the writes.
//   POST /restore {step, ...} rebuilds group "mono-r" from the vault alone, through the install edge's operator routes:
//                             `snapshot` (newest at or before the cutoff, digest checked against the vault log), `tail`
//                             (vault entries after it, in pages, each checked by the edge against the `prev` chain),
//                             `packs` (every referenced pack copied from the vault to the rebuilt install's two buckets,
//                             checksum checked), and `open`.
// Each call is bounded so one invocation stays under the subrequest limit; the runner calls until done.

interface Env { VAULT: R2Bucket; PROV_URL: string; EXPORT_TOKEN: string; M0_TOKEN: string }
interface Entry { idx: number; prev: string; digest: string; at: number; body: string }

const json = (v: unknown, status = 200) => Response.json(v, { status });
const sha256hex = async (s: string) => [...new Uint8Array(await crypto.subtle.digest("SHA-256", new TextEncoder().encode(s)))].map((b) => b.toString(16).padStart(2, "0")).join("");

async function prov<T>(env: Env, path: string, body: unknown, token: string): Promise<T> {
  const r = await fetch(env.PROV_URL + path, { method: "POST", body: JSON.stringify(body), headers: { authorization: `Bearer ${token}` } });
  if (!r.ok) throw new Error(`${path}: ${r.status} ${(await r.text()).slice(0, 300)}`);
  return (path === "/export/object" ? r.text() : r.json()) as Promise<T>;
}
const cursor = async (env: Env) => Number((await (await env.VAULT.get("cursor/mono"))?.text()) ?? "0");

async function pull(env: Env, max: number): Promise<object> {
  const from = (await cursor(env)) + 1;
  const entries = await prov<Entry[]>(env, "/export/entries", { group: "mono", from, max }, env.EXPORT_TOKEN);
  let packs = 0, snaps = 0, bytes = 0;
  // Every pack the entries name, 6 copies at a time; the cursor moves only after all of them are in the bucket.
  const want = entries.flatMap((e) => (JSON.parse(e.body) as { t: string; id?: string; checksum?: string }[]).filter((ev) => ev.t === "pack"));
  let next = 0;
  await Promise.all(Array.from({ length: 6 }, async () => {
    for (let x = next++; x < want.length; x = next++) {
      const ev = want[x], key = `pack/mono/${ev.id}`;
      if (await env.VAULT.head(key)) continue;
      const body = await prov<string>(env, "/export/object", { group: "mono", key }, env.EXPORT_TOKEN);
      if ((await sha256hex(body)) !== ev.checksum) throw new Error(`pack ${ev.id} checksum mismatch`);
      await env.VAULT.put(key, body, { sha256: ev.checksum });
      packs++; bytes += body.length;
    }
  }));
  for (const key of await prov<string[]>(env, "/export/snapshots", { group: "mono" }, env.EXPORT_TOKEN)) {
    if (await env.VAULT.head(key)) continue;
    await env.VAULT.put(key, await prov<string>(env, "/export/object", { group: "mono", key }, env.EXPORT_TOKEN));
    snaps++;
  }
  if (entries.length) {
    await env.VAULT.put(`log/mono/${String(entries[0].idx).padStart(9, "0")}-${String(entries.at(-1)!.idx).padStart(9, "0")}`, JSON.stringify(entries));
    await env.VAULT.put("cursor/mono", String(entries.at(-1)!.idx));
  }
  return { from, entries: entries.length, cursor: await cursor(env), packs, snaps, bytes };
}

async function vaultLog(env: Env, from: number, to: number): Promise<Entry[]> {
  const out: Entry[] = [];
  let token: string | undefined;
  do {
    const page = await env.VAULT.list({ prefix: "log/mono/", cursor: token });
    for (const o of page.objects) {
      const [a, b] = o.key.slice(9).split("-").map(Number);
      if (b < from || a > to) continue;
      out.push(...((await (await env.VAULT.get(o.key))!.json()) as Entry[]).filter((e) => e.idx >= from && e.idx <= to));
    }
    token = page.truncated ? page.cursor : undefined;
  } while (token);
  return out.sort((x, y) => x.idx - y.idx);
}

async function restore(env: Env, b: Record<string, any>): Promise<object> {
  const g = "mono-r", op = env.M0_TOKEN, cutoff: number = b.cutoff;
  if (b.step === "snapshot") {
    const keys = (await env.VAULT.list({ prefix: "snapshot/mono/core/" })).objects.map((o) => o.key);
    const best = keys.map((k) => ({ k, upto: Number(k.split("/").at(-1)!.split("-")[1]) })).filter((x) => x.upto <= cutoff).sort((x, y) => y.upto - x.upto)[0];
    if (!best) return { ok: true, upto: 0, note: "no snapshot; replay from entry 1" };
    const snap = await (await env.VAULT.get(best.k))!.json<{ upto: number; digest: string }>();
    const [logged] = await vaultLog(env, snap.upto, snap.upto);
    if (!logged || logged.digest !== snap.digest) return { ok: false, why: `snapshot ${best.k} does not match vault entry ${snap.upto}` };
    return { key: best.k, ...(await prov<object>(env, "/restore/snapshot", { group: g, snap }, op)) };
  }
  if (b.step === "tail") {
    const entries = await vaultLog(env, b.from, Math.min(cutoff, b.from + b.max - 1));
    return { n: entries.length, ...(await prov<object>(env, "/restore/tail", { group: g, entries }, op)) };
  }
  if (b.step === "packs") {
    const list = await prov<{ id: string; checksum: string }[]>(env, "/restore/packlist", { group: g, offset: b.offset, max: b.max }, op);
    let ok = 0, next = 0;
    const bad: string[] = [];
    // 8 copies at a time; each copy is idempotent (same key, same bytes), so a retried call repeats no harm.
    await Promise.all(Array.from({ length: 8 }, async () => {
      for (let x = next++; x < list.length; x = next++) {
        const p = list[x], body = await (await env.VAULT.get(`pack/mono/${p.id}`))?.text();
        if (body && (await prov<{ ok: boolean }>(env, "/restore/pack", { group: g, id: p.id, body }, op)).ok) ok++;
        else bad.push(p.id);
      }
    }));
    return { n: list.length, ok, bad };
  }
  if (b.step === "open") return prov<object>(env, "/restore/open", { group: g, cutoff }, op);
  return { error: "unknown step" };
}

export default {
  async fetch(req: Request, env: Env): Promise<Response> {
    if (req.headers.get("authorization") !== `Bearer ${env.M0_TOKEN}`) return new Response("unauthorized", { status: 401 });
    const url = new URL(req.url), body = req.method === "POST" ? await req.json<Record<string, any>>() : {};
    try {
      if (url.pathname === "/pull") return json(await pull(env, body.max ?? 200));
      if (url.pathname === "/cursor") return json({ cursor: await cursor(env) });
      if (url.pathname === "/reset") {
        // Harness only: empties the vault bucket before a fresh run (a real vault's bucket lock forbids this).
        let n = 0;
        for (;;) { const page = await env.VAULT.list({ limit: 1000 }); if (!page.objects.length) break; await env.VAULT.delete(page.objects.map((o) => o.key)); n += page.objects.length; }
        return json({ deleted: n });
      }
      if (url.pathname === "/restore") return json(await restore(env, body));
    } catch (e) { return json({ error: String(e) }, 500); }
    return json({ error: "not found" }, 404);
  },
} satisfies ExportedHandler<Env>;
