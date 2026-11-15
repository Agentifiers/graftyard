#!/usr/bin/env python3
"""watch.py REPO_ID FROM_ISO PID CAP_MIB -> every 30 s while PID runs, each voter's analytics memory peak since FROM
(durableObjectsPeriodicGroups max memoryUsageBytes, `repo:REPO_ID#1#%`, as tests/fanout/memstats.py reads it). The
first voter over CAP_MIB stops the run: PID gets SIGTERM, and one line says which voter and when. The operator's stop
line for narrow-reads-3's instrument run (120 MiB). Token from the environment, only as a header."""
import json, os, signal, subprocess, sys, time, urllib.request
from datetime import datetime, timezone

acct, tok = os.environ["CLOUDFLARE_ACCOUNT_ID"], os.environ["CLOUDFLARE_API_TOKEN"]
rid, start, pid, cap = sys.argv[1], sys.argv[2], int(sys.argv[3]), float(sys.argv[4])
H = {"authorization": f"Bearer {tok}", "content-type": "application/json"}
Q = """query($a:String!,$s:Time!,$e:Time!,$n:String!){viewer{accounts(filter:{accountTag:$a}){
 p: durableObjectsPeriodicGroups(limit: 10, orderBy:[max_memoryUsageBytes_DESC], filter:{datetime_geq:$s, datetime_lt:$e, name_like:$n}) {
 max { memoryUsageBytes } dimensions { name } } }}}"""


def alive():
    try:
        os.kill(pid, 0)
        return True
    except OSError:
        return False



# SPEND_FROM and SPEND_CAP: also stop once the install's spend since SPEND_FROM (tests/fanout/m2bcost.py) passes
# SPEND_CAP dollars, checked every 2 minutes.
spend_from, spend_cap, n = os.environ.get("SPEND_FROM"), float(os.environ.get("SPEND_CAP", "0") or 0), 0
while alive():
    now = datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
    n += 1
    if spend_from and spend_cap and n % 4 == 1:
        try:
            out = subprocess.run([sys.executable, os.path.join(os.path.dirname(__file__), "../fanout/m2bcost.py"),
                                  spend_from, now], capture_output=True, text=True, timeout=120).stdout
            usd = json.loads(out.strip().splitlines()[-1])["usd"]
            print(json.dumps({"at": now, "usd": usd}), flush=True)
            if usd > spend_cap:
                print(json.dumps({"at": now, "stop": "spend", "usd": usd, "cap": spend_cap}), flush=True)
                os.kill(pid, signal.SIGTERM)
                break
        except Exception as e:
            print(json.dumps({"at": now, "error": "spend " + str(e)[:100]}), flush=True)
    try:
        v = json.load(urllib.request.urlopen(urllib.request.Request("https://api.cloudflare.com/client/v4/graphql",
            data=json.dumps({"query": Q, "variables": {"a": acct, "s": start, "e": now, "n": f"repo:{rid}#1#%"}}).encode(),
            headers=H), timeout=60))
        rows = v["data"]["viewer"]["accounts"][0]["p"]
        peaks = {r["dimensions"]["name"]: round(r["max"]["memoryUsageBytes"] / 2**20) for r in rows}
        print(json.dumps({"at": now, "voters": peaks}), flush=True)
        over = [(n, m) for n, m in peaks.items() if m > cap]
        if over:
            print(json.dumps({"at": now, "stop": over, "cap": cap}), flush=True)
            os.kill(pid, signal.SIGTERM)
            break
    except Exception as e:
        print(json.dumps({"at": now, "error": str(e)[:120]}), flush=True)
    time.sleep(30)
