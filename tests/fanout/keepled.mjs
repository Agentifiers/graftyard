// LM-1 round 22: keeps a group led during B0-34's drain. The drain's watcher reads only members (/status, /read,
// /stats), which a group does not count as clients, so repo:131296's members idled out to followers at term 2 within
// 3 minutes of m34's drain start and its leader's archive alarm stopped. One edge read a minute is a client.
//   node tests/fanout/keepled.mjs <owner/name> <minutes>
import { api, tokens } from "../am1/hosted/lib.mjs";
const [repo, min = "90"] = process.argv.slice(2);
const end = Date.now() + Number(min) * 60_000;
while (Date.now() < end) {
  const r = await api("GET", `/api/v3/repos/${repo}`, tokens.admin).catch((e) => ({ status: String(e).slice(0, 80) }));
  console.log(JSON.stringify({ at: new Date().toISOString(), status: r.status }));
  await new Promise((r) => setTimeout(r, 60_000));
}
