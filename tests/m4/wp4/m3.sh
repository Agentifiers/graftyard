#!/usr/bin/env bash
# R23 release M3's hosted half (decisions/DECISIONS.md, `r23-m3`; tests/m4/results/m4-wp4.md, "R23 release M3"):
#   bash tests/m4/wp4/m3.sh before   the legacy classes' Workers Logs since $SINCE and both accounts' instance lists;
#                                    exit 3 if any legacy object is live (armed with no end, or an instance not stopped)
#   bash tests/m4/wp4/m3.sh init     `graftyard runners init` from $BUNDLE (migration v2 to v3, the two Sandbox
#                                    applications deleted); appended to tests/m4/raw/wp4/runners-init.log. It keeps
#                                    the stored HostedRunnersAppKey unless APP_KEY names a key file the App accepts
#                                    (`init-key-guard`): ~/.graft-m4a/hosted/app.pem is the retired key 1
#   bash tests/m4/wp4/m3.sh burst    tests/m4/wp4/r23.mjs, 8 jobs on RunnerBox, into tests/m4/raw/wp4/m3/
#   bash tests/m4/wp4/m3.sh after    only graft-runners-runnerbox listed, the script at v3, nothing running
# Needs CLOUDFLARE_RUNNER_ACCOUNT_ID and CLOUDFLARE_RUNNER_API_TOKEN (the runner account), `tests/am1/lf13/env-m4a.sh`
# for the burst, and the graftyard CLI built from main with r23-m3. v3 has no rollback: `init` refuses before writing
# anything while a Sandbox container is not stopped, and `before` stops the row first.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
STEP=${1:?before|init|burst|after}
SINCE=${SINCE:-2026-11-26T06:00:00Z}
HOSTED=${HOSTED:-$HOME/.graft-m4a/hosted}
BUNDLE=${BUNDLE:-/tmp/m3-bundle}
IMAGE=${IMAGE:?the runner image by digest, as RunnerBox runs it now}
OUT=tests/m4/raw/wp4/m3
mkdir -p "$OUT"

case "$STEP" in
before | after)
  STEP=$STEP SINCE=$SINCE python3 - <<'EOF' | tee -a "$OUT/$STEP.json"
import json, os, sys, time, urllib.request
from datetime import datetime, timezone
a, t = os.environ["CLOUDFLARE_RUNNER_ACCOUNT_ID"], os.environ["CLOUDFLARE_RUNNER_API_TOKEN"]
def call(path, body=None):
    r = urllib.request.Request(f"https://api.cloudflare.com/client/v4/accounts/{a}/{path}",
                               data=None if body is None else json.dumps(body).encode(), method="GET" if body is None else "POST",
                               headers={"Authorization": "Bearer " + t, "content-type": "application/json"})
    return json.load(urllib.request.urlopen(r, timeout=60))["result"]
ms = lambda s: int(datetime.fromisoformat(s.replace("Z", "+00:00")).timestamp() * 1000)
lo, hi = ms(os.environ["SINCE"]), int(time.time() * 1000)
def events(filters):
    out, top = [], hi
    while True:
        e = call("workers/observability/telemetry/query", {"queryId": "m3", "timeframe": {"from": lo, "to": top},
            "view": "events", "limit": 100, "parameters": {"filters": [
                {"key": "$metadata.service", "operation": "eq", "type": "string", "value": "graft-runners"}] + filters}})
        e = e.get("events", {}).get("events", [])
        out += e
        if len(e) < 100:
            return out
        top = min(x["timestamp"] for x in e)
legacy = {}
for cls in ["SandboxStd3", "SandboxStd4"]:
    objs = {}
    for x in events([{"key": "$workers.entrypoint", "operation": "eq", "type": "string", "value": cls}]):
        w, m = x.get("$workers") or {}, x["$metadata"]
        s = objs.setdefault(w.get("durableObjectId"), {"arm": 0, "end": 0})
        rpc = (w.get("event") or {}).get("rpcMethods") or []
        s["arm"] += "arm" in rpc
        s["end"] += "destroy" in rpc or (m.get("message") or "").startswith("graft-runners: launched") or "left to go stale" in (m.get("message") or "")
    legacy[cls] = {"objects": len(objs), "armed_no_end": sum(1 for s in objs.values() if s["arm"] and not s["end"])}
passes = [x["$metadata"]["message"] for x in events([
    {"key": "$metadata.message", "operation": "includes", "type": "string", "value": "graft-runners pass"},
    {"key": "$metadata.message", "operation": "not_includes", "type": "string", "value": "destroyed 0"}])]
apps = {}
for app in call("containers/applications"):
    ins = call(f"containers/applications/{app['id']}/instances?per_page=1000").get("instances", [])
    st = {}
    for i in ins:
        k = (i.get("status") or {}).get("state", "unknown")
        st[k] = st.get(k, 0) + 1
    apps[app["name"]] = st
tag = next((s.get("migration_tag") for s in call("workers/scripts") if s["id"] == "graft-runners"), None)
dep = call("workers/scripts/graft-runners/deployments")["deployments"][0]
done = {"inactive", "stopped", "failed", "deleted"}
legacy_live = sum(n for name, st in apps.items() if "sandbox" in name for k, n in st.items() if k not in done)
live = sum(n for st in apps.values() for k, n in st.items() if k not in done)
r = {"step": os.environ["STEP"], "at": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"), "since": os.environ["SINCE"],
     "legacy_events": legacy, "passes_destroying": passes, "apps": apps, "migration_tag": tag,
     "deployment": [(v["version_id"][:8], v["percentage"]) for v in dep["versions"]], "legacy_live": legacy_live, "live": live}
if os.environ["STEP"] == "before":
    r["stop"] = legacy_live > 0 or any(v["armed_no_end"] for v in legacy.values())
else:
    r["pass"] = list(apps) == ["graft-runners-runnerbox"] and tag == "v3" and live == 0
print(json.dumps(r))
sys.exit(3 if r.get("stop") or r.get("pass") is False else 0)
EOF
  ;;
init)
  {
    echo "== R23 M3 $(date -u +%FT%TZ) from $(git rev-parse --short HEAD), bundle $BUNDLE, image ${IMAGE##*@}"
    cargo run -q -p graftyard -- runners init --account "$CLOUDFLARE_RUNNER_ACCOUNT_ID" --spec "$HOSTED/runners.json" \
      --bundle "$BUNDLE" --image "$IMAGE" ${APP_KEY:+--app-key "$APP_KEY"} --webhook-secret "$HOSTED/webhook.secret" 2>&1 \
      && echo "exit 0" || { echo "exit $?"; exit 1; }
  } | tee -a tests/m4/raw/wp4/runners-init.log
  ;;
burst)
  # shellcheck disable=SC1091
  source tests/am1/lf13/env-m4a.sh
  node tests/m4/wp4/r23.mjs "$OUT" 2>&1 | tee "$OUT/run.log"
  ;;
*) echo "unknown step $STEP" >&2; exit 2 ;;
esac
