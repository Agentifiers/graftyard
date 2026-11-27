#!/usr/bin/env bash
# graft-controller's own build answers a `graftyard_task` delivery from graft-agents-front in local workerd (F144).
# Builds workers/controller (test-faults, or the product build with PRODUCT=1), serves it with a local Secrets Store
# webhook secret and the TaskBox binding, and sends signed deliveries with and without `x-graftyard-agents-front`. A
# `cancelled` delivery for a task with no box answers the box's own 404; with the front header the response still
# carries `x-graftyard-controller-version`. Before F144's fix the front's copy answered 500 "Can't modify immutable
# headers", because the version report was set on the TaskBox stub's response. Exits 1 on any 500 or a missing
# version report. Run from the repository root.
set -uo pipefail
root=$(cd "$(dirname "$0")/../.." && pwd); P=${P:-8797}; W=$root/tests/m0/node_modules/.bin/wrangler
B=$(mktemp -d)
feat=(--features test-faults); [ -n "${PRODUCT:-}" ] && feat=()
cd "$root/workers/controller" && worker-build --release --out-dir "$B/build" "${feat[@]}" >"$B/build.log" 2>&1 \
  || { tail -20 "$B/build.log"; exit 1; }
cp "$root/workers/controller/entry.mjs" "$B/"
cat >"$B/wrangler.json" <<'EOF'
{"name": "ctlboot", "main": "entry.mjs", "compatibility_date": "2026-10-01", "version_metadata": {"binding": "VERSION"},
 "secrets_store_secrets": [{"binding": "WEBHOOK_SECRET", "store_id": "ctlboot", "secret_name": "hook"}],
 "durable_objects": {"bindings": [{"name": "TASKBOX", "class_name": "TaskBox"}]},
 "migrations": [{"tag": "v1", "new_sqlite_classes": ["TaskBox"]}]}
EOF
cd "$B"
echo -n ctlboot-hook | "$W" secrets-store secret create ctlboot --name hook --scopes workers --persist-to "$B/state" \
  -c wrangler.json >"$B/secret.log" 2>&1 || { cat "$B/secret.log"; exit 1; }
"$W" dev --port $P -c wrangler.json --persist-to "$B/state" >"$B/dev.log" 2>&1 & DEV=$!
trap 'kill $DEV 2>/dev/null; wait $DEV 2>/dev/null; rm -rf "$B"' EXIT
for _ in $(seq 90); do grep -q "Ready on" "$B/dev.log" && break; sleep 1; done
python3 - "$P" <<'PY'
import hashlib, hmac, json, sys, urllib.error, urllib.request
port = sys.argv[1]
body = json.dumps({"action": "cancelled", "repository": {"full_name": "o/r"},
                   "task": {"kind": "graftyard_run", "id": "01a219a6-0ad2-8d62-b501-6399710e6731",
                            "requester": {"id": 7}, "state": "cancelled"}}).encode()
sig = "sha256=" + hmac.new(b"ctlboot-hook", body, hashlib.sha256).hexdigest()
def send(delivery, front):
    h = {"x-hub-signature-256": sig, "x-github-event": "graftyard_task", "x-github-delivery": delivery,
         "content-type": "application/json"}
    if front:
        h["x-graftyard-agents-front"] = "1"
    req = urllib.request.Request(f"http://127.0.0.1:{port}/", data=body, headers=h, method="POST")
    try:
        r = urllib.request.urlopen(req, timeout=30)
    except urllib.error.HTTPError as e:
        r = e
    return r.status, r.headers.get("x-graftyard-controller-version"), r.read()[:200]
fail = []
for delivery, front in (("ctlboot-1", False), ("ctlboot-2", True)):
    code, version, text = send(delivery, front)
    print(f"graftyard_task cancelled, front {front}: {code} version {version} {text!r}")
    if code != 404 or b"no box" not in text:
        fail.append(f"front {front}: {code}, not the box's 404")
    if front and not version:
        fail.append("front: no x-graftyard-controller-version")
print("  FAIL: " + "; ".join(fail) if fail else "deliveries ok")
raise SystemExit(1 if fail else 0)
PY
ok=$?
n=$(grep -c "immutable headers\|RuntimeError" "$B/dev.log")
echo "immutable-header errors and traps: $n"; [ "$n" = 0 ] || ok=1
[ $ok = 0 ] && echo PASS || { echo FAIL; exit 1; }
