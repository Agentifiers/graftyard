// BF-46's live traffic: every 10 s, one installation token for m3-ci on m3org through the edge (`OPS` ->
// `graft-m4a-ops` `/mint`), then one read with it, so a failed call or a token under the wrong key shows the moment a
// slot switches. One JSON line per mint in <out>: the time, the mint's status and ms, the token's `kid` (from its JWS
// header), and the read's status. No token is written. Runs until killed.
//   node tests/m5/bf46/mint.mjs <out.jsonl>
import { api, ghs, row, sleep } from "../../am1/hosted/lib.mjs";

const out = process.argv[2];
for (;;) {
  const t0 = Date.now();
  const r = await ghs();
  const line = { at: new Date(t0).toISOString(), status: r.status, ms: r.ms };
  if (r.status === 201) {
    const jws = r.body.token.split("_").slice(2).join("_");
    line.kid = JSON.parse(Buffer.from(jws.split(".")[0], "base64url").toString()).kid;
    const g = await api("GET", "/api/v3/repos/m3org/j4", r.body.token);
    line.read = g.status;
  } else {
    line.error = JSON.stringify(r.body).slice(0, 160);
  }
  row(out, line);
  await sleep(Math.max(0, 10_000 - (Date.now() - t0)));
}
