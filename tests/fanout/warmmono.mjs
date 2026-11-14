// LM-1 round 17b: polls ls-refs of m3org/b034-mono through the edge until main appears or `secs` pass; one line per try.
//   node tests/fanout/warmmono.mjs [seconds 900]   (environment as tests/am1/hosted/lib.mjs; no token reaches a log)
const { tokens } = await import("../am1/hosted/lib.mjs");
const secs = Number(process.argv[2] ?? 900);
const until = Date.now() + secs * 1000;
const pkt = (s) => { const b = Buffer.from(s); return Buffer.concat([Buffer.from((b.length + 4).toString(16).padStart(4, "0")), b]); };
const body = Buffer.concat([pkt("command=ls-refs\n"), Buffer.from("0001"), pkt("ref-prefix refs/heads/main\n"), Buffer.from("0000")]);
while (Date.now() < until) {
  const t = Date.now();
  const r = await fetch(`${process.env.GRAFT_EDGE}/m3org/b034-mono.git/git-upload-pack`, { method: "POST", body,
    headers: { authorization: `token ${tokens.admin}`, "user-agent": "git/2.56.0", "content-type": "application/x-git-upload-pack-request", "git-protocol": "version=2" },
    signal: AbortSignal.timeout(90_000) }).catch((e) => ({ status: 0, text: async () => String(e) }));
  const text = (await r.text()).slice(0, 160).replace(/[0-9a-f]{40}/g, "<oid>");
  const ok = text.includes("refs/heads/main");
  console.log(JSON.stringify({ at: new Date(t).toISOString(), status: r.status, ms: Date.now() - t, ok, text: ok ? "main" : text }));
  if (ok) break;
  await new Promise((s) => setTimeout(s, 10_000));
}
