#!/usr/bin/env python3
"""purge.py [--dry-run] NET [NET ...] -> deletes a retired test network's objects from every site bucket of graft-m2b (or
$INSTALL), after its group was retired with `/retire` (operator, R25: retired test groups are test-harness state, so the
harness cleans their packs up; the sweeper never treats a silent group as gone).

Gate, per network: the repository chain `repo:NET` must answer "not joined" on every voter its registry rows name on
each site and on each site's first-generation voter; a network with any member joined, or any status that failed, is
refused and nothing of it is deleted. Then every key under the network's prefixes (`prefixes`) is listed and deleted
in batches of 1,000 through R2's S3 API, and listed again. A live purge runs twice, waiting for sweep runs that started
before the first pass to finish; the log names both passes' per-bucket counts before and after. A dry run makes one pass.

Run with `uv run -q --with boto3 python3 tests/fanout/purge.py ...`. Needs CLOUDFLARE_ACCOUNT_ID, CLOUDFLARE_API_TOKEN
(R2 write; its S3 credentials are the token's id and the SHA-256 of its value), GRAFT_TOOLS_EDGE, and the tools token
file (TOOLS_TOKEN_FILE, default tests/m0/.m0-token in the main checkout); tokens go in headers only, never on argv or
in a log. Log: $PURGE_LOG (default tests/fanout/raw/purge-m2b.log)."""
import datetime, hashlib, json, os, sys, time, urllib.request

INSTALL = os.environ.get("INSTALL", "graft-m2b")
SITES = "abc"
# Each site's pack bucket: graft-m2b's are `graft-m2b-site-a-packs`; graft-m1's are `graft-m1-packs-a`
# (tests/m3/finish/sites4.json), so B0-30 runs it with BUCKET='{install}-packs-{site}'.
BUCKET = os.environ.get("BUCKET", "{install}-site-{site}-packs")

SWEEP_SLOT_S = 900  # Site sweeps start at */15-minute cron boundaries.
SWEEP_WALL_S = 480  # crates/git/src/sweep.rs SWEEP_WALL stops new work after 8 minutes.
SETTLE_S = 120  # Two minutes for work already in flight when SWEEP_WALL expires.


def second_pass_at(t):
    """When every sweep run that started at or before `t` has ended: the latest such run started at `t`'s */15
    boundary, stops starting work SWEEP_WALL later, and finishes what it started within SETTLE_S."""
    return t // SWEEP_SLOT_S * SWEEP_SLOT_S + SWEEP_WALL_S + SETTLE_S


def purge_twice(purge_one, nets, now, sleep):
    """Purges every network, waits until `second_pass_at` the first listing, and purges again: a sweep run that started
    before the first pass re-put 165 `committed/pack/131349/` markers after it on m43 (R25). Returns (net, first
    counts, second counts) per network."""
    t0 = now()
    first = [(net, purge_one(net)) for net in nets]
    if not first:
        return []
    deadline = second_pass_at(t0)
    if deadline > now():
        sleep(deadline - now())
    second = [(net, purge_one(net)) for net in nets]
    return [(net, first_counts, second_counts)
            for (net, first_counts), (_, second_counts) in zip(first, second)]


def prefixes(net):
    """Every key prefix one network's objects live under (`crates/git/src/key.rs`, `R2Key`'s `Display`): the trailing
    `/` keeps network 13132 from matching 131322."""
    n = int(net)
    by_net = ["pack", "committed/pack", "upload", "compose", "clone", "archive-cache", "blob", "committed/blob"]
    by_chain = ["archive", "snapshot", "inline-idx"]
    return [f"{p}/{n}/" for p in by_net] + [f"{p}/{c}:{n}/" for p in by_chain for c in ("repo", "repo-meta")]


def retired(answers):
    """The gate: every answer is a status that says "not joined", and there is at least one per site."""
    if not answers or {s for s, _ in answers} != {0, 1, 2}:
        return False
    return all(isinstance(v, dict) and v.get("joined") is False for _, v in answers)


def purge(s3, bucket, net, dry):
    """Lists and deletes every key under `net`'s prefixes in `bucket`. Returns (before, after)."""
    keys = []
    for p in prefixes(net):
        for page in s3.get_paginator("list_objects_v2").paginate(Bucket=bucket, Prefix=p):
            keys += [o["Key"] for o in page.get("Contents", [])]
    if not dry:
        for i in range(0, len(keys), 1000):
            r = s3.delete_objects(Bucket=bucket, Delete={"Objects": [{"Key": k} for k in keys[i:i + 1000]], "Quiet": True})
            if r.get("Errors"):
                raise RuntimeError(f"{bucket}: {len(r['Errors'])} deletes failed: {r['Errors'][0]}")
    after = 0 if dry else sum(len(page.get("Contents", [])) for p in prefixes(net)
                              for page in s3.get_paginator("list_objects_v2").paginate(Bucket=bucket, Prefix=p))
    return len(keys), (len(keys) if dry else after)


def main():
    import boto3
    args = sys.argv[1:]
    dry = "--dry-run" in args
    nets = [int(a) for a in args if not a.startswith("--")]
    if not nets:
        sys.exit(__doc__)
    acct, tok = os.environ["CLOUDFLARE_ACCOUNT_ID"], os.environ["CLOUDFLARE_API_TOKEN"]
    tools = os.environ["GRAFT_TOOLS_EDGE"]
    main_checkout = os.environ.get("GRAFT_MAIN_CHECKOUT", "/home/user/workspace/repo")
    ttok = open(os.environ.get("TOOLS_TOKEN_FILE", f"{main_checkout}/tests/m0/.m0-token")).read().strip()
    log_path = os.environ.get("PURGE_LOG", "tests/fanout/raw/purge-m2b.log")

    def log(s):
        line = f"{datetime.datetime.now(datetime.UTC).strftime('%Y-%m-%dT%H:%M:%SZ')} {s}"
        assert tok not in line and ttok not in line
        print(line, flush=True)
        with open(log_path, "a") as f:
            f.write(line + "\n")

    def call(path, body):
        r = urllib.request.Request(f"{tools}{path}", data=json.dumps(body).encode(), method="POST", headers={
            "authorization": f"Bearer {ttok}", "content-type": "application/json", "user-agent": "graftyard-am1"})
        return json.load(urllib.request.urlopen(r, timeout=60))

    def status(chain, site, do_name):
        try:
            return call("/member", {"chain": chain, "site": site, "path": "/status", "deadline": 15_000, "gen": 1,
                                    "do_name": do_name}).get("v")
        except Exception as e:
            return {"error": str(e)[:80]}

    v = json.load(urllib.request.urlopen(urllib.request.Request(
        f"https://api.cloudflare.com/client/v4/accounts/{acct}/tokens/verify", headers={"authorization": f"Bearer {tok}"}),
        timeout=30))
    s3 = boto3.client("s3", endpoint_url=f"https://{acct}.r2.cloudflarestorage.com", region_name="auto",
                      aws_access_key_id=v["result"]["id"], aws_secret_access_key=hashlib.sha256(tok.encode()).hexdigest())
    totals = {"purged": [], "refused": []}
    gated = []
    for net in nets:
        chain = f"repo:{net}"
        answers = []
        for site in range(3):
            names = {f"{chain}#1#f{site}"}
            try:
                row = call("/registry", {"chain": chain, "site": site})
                if isinstance(row, dict) and row.get("do_name"):
                    names.add(row["do_name"])
            except Exception as e:
                answers.append((site, {"error": f"registry: {str(e)[:60]}"}))
            answers += [(site, status(chain, site, n)) for n in sorted(names)]
        if not retired(answers):
            live = [f"{s}:{a.get('role') or a.get('error')}" for s, a in answers if not (isinstance(a, dict) and a.get("joined") is False)]
            log(f"{net} REFUSED: not retired ({', '.join(live)[:160]})")
            totals["refused"].append(net)
            continue
        gated.append(net)

    second = 0
    if dry:
        for net in gated:
            counts = {site: purge(s3, BUCKET.format(install=INSTALL, site=site), net, True) for site in SITES}
            log(f"{net} would delete: " + ", ".join(f"{s} {b} -> {a}" for s, (b, a) in counts.items()))
            totals["purged"].append([net, {s: b for s, (b, _) in counts.items()}])
    else:
        def logged_sleep(s):
            until = datetime.datetime.fromtimestamp(time.time() + s, datetime.UTC).isoformat()
            log(f"waiting {int(s)} s for sweep runs started before the first pass (until {until})")
            time.sleep(s)

        results = purge_twice(
            lambda net: {site: purge(s3, BUCKET.format(install=INSTALL, site=site), net, False) for site in SITES},
            gated, now=time.time, sleep=logged_sleep)
        for net, first_counts, second_counts in results:
            log(f"{net} purged: " + ", ".join(f"{s} {b} -> {a}" for s, (b, a) in first_counts.items()))
            log(f"{net} second pass: " + ", ".join(f"{s} {b} -> {a}" for s, (b, a) in second_counts.items()))
            totals["purged"].append([net, {s: b for s, (b, _) in first_counts.items()}])
            second += sum(b for b, _ in second_counts.values())
    log(json.dumps({"dry": dry, "purged": len(totals["purged"]), "objects": sum(sum(c.values()) for _, c in totals["purged"]),
                    "refused": totals["refused"], "second": second}))


if __name__ == "__main__":
    main()
