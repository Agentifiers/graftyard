// `DELETE /applications/{client}/token` for the token in <token file>, as gh's own client (a public client, whose
// embedded secret the edge ignores). Prints the status.
//   node tests/m3/hosted/revoke.mjs <token file> <client id>
import { readFileSync } from "node:fs";
const [file, client] = process.argv.slice(2);
const EDGE = process.env.GRAFT_EDGE ?? "https://graft-m1-edge.<workers-subdomain>.workers.dev";
const r = await fetch(`${EDGE}/api/v3/applications/${client}/token`, {
  method: "DELETE",
  headers: { authorization: `Basic ${Buffer.from(`${client}:x`).toString("base64")}`, "content-type": "application/json" },
  body: JSON.stringify({ access_token: readFileSync(file, "utf8").trim() }),
});
console.log(r.status);
