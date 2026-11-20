// The SSH-off guard for site and edge deploys (tests/m6/results/m6-wp2.md, "Cutover"). A deploy without `--ssh` drops
// the `SshFront` class binding and `SSH_OPTION=on`, which turns native SSH off for every hostname whose Spectrum app
// has this install's edge as its Worker origin. So a deploy without `--ssh` first lists the account's Spectrum apps
// through the Cloudflare API, not a local file, and refuses while any app's origin is this install: the edge script
// itself, or a script that binds `SshFront` on one of the install's site scripts. `--ssh-off` deploys anyway. An API
// error refuses too, since the guard cannot tell. The token is the deploy's own `CLOUDFLARE_API_TOKEN`, which needs
// zone read and Spectrum read on the account's zones.

// The hostnames of Spectrum apps whose Worker origin is this install. `api(path)` is a parsed Cloudflare API GET
// relative to `/client/v4/`, throwing on a failed call. `edge` is the edge script being deployed, or null for a site
// deploy; `siteScripts` are the install's site scripts.
export async function sshHostnames(api, account, { edge, siteScripts }) {
  const zones = (await api(`zones?account.id=${account}&per_page=50`)).result;
  const apps = (await Promise.all(zones.map(async (z) => (await api(`zones/${z.id}/spectrum/apps?per_page=100`)).result ?? [])))
    .flat()
    .filter((a) => a.traffic_type === "worker" && a.origin_worker_id);
  if (!apps.length) return [];
  const byTag = new Map((await api(`accounts/${account}/workers/scripts`)).result.map((s) => [s.tag, s.id]));
  const ours = new Set(siteScripts);
  const hits = [];
  for (const a of apps) {
    const script = byTag.get(a.origin_worker_id);
    if (!script) continue;
    let mine = script === edge;
    if (!mine) {
      const bindings = (await api(`accounts/${account}/workers/scripts/${script}/settings`)).result?.bindings ?? [];
      mine = bindings.some((b) => b.class_name === "SshFront" && ours.has(b.script_name));
    }
    if (mine) hits.push(`${a.dns?.name} (origin ${script})`);
  }
  return hits;
}

// Throws unless the deploy keeps SSH on (`--ssh`), opts out (`--ssh-off`), or no Spectrum app points at the install.
export async function refuseSshOff(flags, { edge = null, siteScripts }, api = cfApi) {
  if (flags.includes("--ssh") || flags.includes("--ssh-off")) return;
  let hits;
  try {
    hits = await sshHostnames(api, process.env.CLOUDFLARE_ACCOUNT_ID, { edge, siteScripts });
  } catch (e) {
    throw new Error(`cannot list Spectrum apps to check that this deploy keeps SSH on (${e.message}); pass --ssh, or --ssh-off to turn SSH off`);
  }
  if (hits.length) {
    throw new Error(`a deploy without --ssh turns native SSH off for ${hits.join(", ")}; pass --ssh to keep it, or --ssh-off to turn it off`);
  }
}

async function cfApi(path) {
  const r = await fetch(`https://api.cloudflare.com/client/v4/${path}`, {
    headers: { authorization: `Bearer ${process.env.CLOUDFLARE_API_TOKEN}` },
  });
  const body = await r.json().catch(() => ({}));
  if (!r.ok || body.success === false) throw new Error(`GET ${path.split("?")[0]}: ${r.status} ${body.errors?.[0]?.message ?? ""}`.trim());
  return body;
}
