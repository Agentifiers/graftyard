#!/usr/bin/env python3
"""LM-3's kill row's scripted harness (`build/BUILD-PLAN.md`, AM3-WP1): one Graftyard-run task's work, as a Sandbox runs
it under `graft-controller`'s `TaskBox`. It clones the task's base through the outbound router, checks out
`GRAFTYARD_START` (the last acked push, empty for the base), and every `INTERVAL` seconds commits one file
`work/<n>.txt` and pushes it to `refs/heads/graftyard/<task>/work`, until the branch holds `PUSHES` commits; then it
reports `Finished` to the box. Each commit's message names the Sandbox and its wall time, so the row reads from the
branch which Sandbox made each push and when.

Environment, from the box (`GRAFTYARD_SANDBOX`, `GRAFTYARD_START`) and the image's entrypoint arguments:
    python3 /harness.py <host> <owner>/<repo> [pushes] [interval s]
The container's CA is per container (`tests/m0/results/m0-wp14.md`, item 6), so git and curl are pointed at it. No
credential is held here: the router attaches the task's tokens.
"""
import json
import os
import subprocess
import sys
import time
import urllib.request

host, repo = sys.argv[1], sys.argv[2]
pushes = int(sys.argv[3]) if len(sys.argv) > 3 else 8
interval = float(sys.argv[4]) if len(sys.argv) > 4 else 15
sandbox = os.environ["GRAFTYARD_SANDBOX"]
start = os.environ.get("GRAFTYARD_START", "")
task = sandbox.rsplit("-", 1)[0]
ref = f"refs/heads/graftyard/{task}/work"
CA = "/etc/cloudflare/certs/cloudflare-containers-ca.crt"
env = dict(os.environ, GIT_SSL_CAINFO=CA, GIT_TERMINAL_PROMPT="0", GIT_AUTHOR_NAME="lm3-harness",
           GIT_AUTHOR_EMAIL="lm3@graftyard.invalid", GIT_COMMITTER_NAME="lm3-harness",
           GIT_COMMITTER_EMAIL="lm3@graftyard.invalid")
url = f"https://{host}/{repo}.git"


def git(*a, check=True):
    r = subprocess.run(["git", *a], cwd="/work/r", env=env, capture_output=True, text=True)
    if check and r.returncode:
        raise SystemExit(f"git {a[0]}: {r.stderr.strip()[-300:]}")
    return r.stdout.strip()


def report(event):
    ctx = __import__("ssl").create_default_context(cafile=CA)
    req = urllib.request.Request("https://controller.graftyard.internal/report", data=json.dumps(event).encode(),
                                 method="POST", headers={"content-type": "application/json"})
    with urllib.request.urlopen(req, context=ctx, timeout=30) as r:
        print(f"report {event}: {r.status}", flush=True)


for attempt in range(30):
    if subprocess.run(["git", "clone", "-q", url, "/work/r"], env=env).returncode == 0:
        break
    time.sleep(5)
else:
    raise SystemExit("clone failed")
if start:
    # The last acked push is on the task's branch; fetch it, since a fresh clone of the base may not hold it.
    # The last acked push is on the task's branch, or was before a push the box never saw acked replaced it; fetch the
    # branch and the commit itself, then push with force from it (a later unacked push is lost work, not history).
    git("fetch", "-q", "origin", f"+{ref}:refs/remotes/origin/work")
    git("fetch", "-q", "origin", start, check=False)
    git("checkout", "-q", start)
done = int(git("rev-list", "--count", "HEAD")) - int(git("rev-list", "--count", "origin/HEAD"))
print(f"{sandbox} from {start or 'base'}: {done} of {pushes} pushes done", flush=True)
while done < pushes:
    time.sleep(interval)
    done += 1
    os.makedirs("/work/r/work", exist_ok=True)
    # 256 KiB of noise per push, so a push lasts long enough for the row to kill a Sandbox during one.
    open(f"/work/r/work/{done}.txt", "w").write(f"{done}\n{os.urandom(131072).hex()}\n")
    git("add", "work")
    git("commit", "-q", "-m", f"push {done} by {sandbox} at {time.time():.3f}")
    for t in range(5):
        r = subprocess.run(["git", "push", "-q", "origin", f"+HEAD:{ref}"], cwd="/work/r", env=env,
                           capture_output=True, text=True)
        if r.returncode == 0:
            print(f"push {done} ok at {time.time():.3f}", flush=True)
            break
        print(f"push {done} try {t}: {r.stderr.strip()[-200:]}", flush=True)
        time.sleep(3)
    else:
        raise SystemExit(f"push {done} failed")
# LM-3's in-repo row holds a task live while `refs/heads/hold/<task>` exists on the base (the row pushes and deletes
# it), at most 50 minutes. Each read goes through the router, so it counts as the Sandbox's activity (SILENT_MAX).
hold, t0 = f"refs/heads/hold/{task}", time.time()
while time.time() - t0 < 50 * 60:
    r = subprocess.run(["git", "ls-remote", url, hold], env=env, capture_output=True, text=True)
    if r.returncode == 0 and not r.stdout.strip():
        break
    print(f"held by {hold} ({r.returncode})", flush=True)
    time.sleep(20)
report("Finished")
time.sleep(3600)
