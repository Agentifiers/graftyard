#!/usr/bin/env python3
"""LM-3's kill row, real-harness case: one Graftyard-run task's work done by the stock Claude Code CLI in a Sandbox
under `graft-controller`'s `TaskBox`. As the scripted harness (`../harness.py`) does, it clones the task's base through
the outbound router, checks out `GRAFTYARD_START` (the last acked push, empty for the base), and builds the task's
branch `refs/heads/graftyard/<task>/work` one step at a time until it holds `PUSHES` commits. Each step is one
`claude -p` turn that writes `work/<n>.md`; this script commits it, names the Sandbox and wall time in the message, and
pushes. Then it reports `Finished` to the box. After every turn it reports `Used` with the task's model tokens so far,
as Claude Code's own result counts them.

Claude Code talks to the model at `ANTHROPIC_BASE_URL` (the operator's token proxy, `/v1/messages`) through the router, which drops the dummy
`ANTHROPIC_API_KEY` and sets the model key. It may use only `Read`, `Write` and `Edit`: git stays here, so every push
goes through this script under the task's prefix.

    python3 /run.py <host> <owner>/<repo> [pushes] [model]
"""
import json
import os
import subprocess
import sys
import time
import urllib.request

host, repo = sys.argv[1], sys.argv[2]
pushes = int(sys.argv[3]) if len(sys.argv) > 3 else 6
model = sys.argv[4] if len(sys.argv) > 4 else os.environ.get("ANTHROPIC_MODEL", "claude-sonnet-5-5")
sandbox = os.environ["GRAFTYARD_SANDBOX"]
start = os.environ.get("GRAFTYARD_START", "")
task = sandbox.rsplit("-", 1)[0]
ref = f"refs/heads/graftyard/{task}/work"
CA = "/etc/cloudflare/certs/cloudflare-containers-ca.crt"
env = dict(os.environ, GIT_SSL_CAINFO=CA, GIT_TERMINAL_PROMPT="0", GIT_AUTHOR_NAME="lm3-claude",
           GIT_AUTHOR_EMAIL="lm3@graftyard.invalid", GIT_COMMITTER_NAME="lm3-claude",
           GIT_COMMITTER_EMAIL="lm3@graftyard.invalid")
url = f"https://{host}/{repo}.git"
t0 = time.time()
tokens = 0


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
        print(f"report {json.dumps(event)[:120]}: {r.status}", flush=True)


def turn(n):
    """One `claude -p` turn writing work/<n>.md: the CLI's exit code and the tokens its result counts."""
    prompt = (f"In this repository, create the file work/{n}.md: a heading 'Step {n}', then a short Python function "
              f"`step_{n}` in a fenced code block that returns {n} squared, then one sentence on what it does. "
              "Write only that file. Do not run commands.")
    r = subprocess.run(["claude", "-p", prompt, "--model", model, "--output-format", "json", "--max-turns", "4",
                        "--allowedTools", "Read,Write,Edit", "--dangerously-skip-permissions"],
                       cwd="/work/r", env=env, capture_output=True, text=True, timeout=600)
    try:
        out = json.loads(r.stdout)
    except ValueError:
        print(f"turn {n}: exit {r.returncode}, no JSON: {(r.stdout + r.stderr)[-400:]}", flush=True)
        return r.returncode or 1, 0
    u = out.get("usage") or {}
    used = sum(int(u.get(k) or 0) for k in ("input_tokens", "output_tokens", "cache_creation_input_tokens",
                                             "cache_read_input_tokens"))
    print(f"turn {n}: exit {r.returncode}, {out.get('subtype')}, {used} tokens, cost_usd {out.get('total_cost_usd')}",
          flush=True)
    return r.returncode, used


for attempt in range(30):
    if subprocess.run(["git", "clone", "-q", url, "/work/r"], env=env).returncode == 0:
        break
    time.sleep(5)
else:
    raise SystemExit("clone failed")
if start:
    # The last acked push is on the task's branch, or was before a push the box never saw acked replaced it; fetch the
    # branch and the commit itself, then push with force from it (a later unacked push is lost work, not history).
    git("fetch", "-q", "origin", f"+{ref}:refs/remotes/origin/work")
    git("fetch", "-q", "origin", start, check=False)
    git("checkout", "-q", start)
done = int(git("rev-list", "--count", "HEAD")) - int(git("rev-list", "--count", "origin/HEAD"))
print(f"{sandbox} from {start or 'base'}: {done} of {pushes} pushes done", flush=True)
while done < pushes:
    n = done + 1
    code, used = turn(n)
    tokens += used
    report({"Used": {"task_tokens": tokens, "org_month_tokens": tokens,
                     "elapsed": {"secs": int(time.time() - t0), "nanos": 0}}})
    path = f"/work/r/work/{n}.md"
    if not os.path.exists(path):
        # The model wrote nothing: the step still lands, so the branch's count stays the row's clock.
        os.makedirs("/work/r/work", exist_ok=True)
        open(path, "w").write(f"# Step {n}\n\n(no model output)\n")
    git("add", "work")
    # The CLI's exit code rides the commit message, so the row reads every turn's from the work branch.
    git("commit", "-q", "-m", f"push {n} by {sandbox} at {time.time():.3f} claude exit {code} tokens {used}")
    for t in range(5):
        r = subprocess.run(["git", "push", "-q", "origin", f"+HEAD:{ref}"], cwd="/work/r", env=env,
                           capture_output=True, text=True)
        if r.returncode == 0:
            print(f"push {n} ok at {time.time():.3f}", flush=True)
            break
        print(f"push {n} try {t}: {r.stderr.strip()[-200:]}", flush=True)
        time.sleep(3)
    else:
        raise SystemExit(f"push {n} failed")
    done = n
report("Finished")
time.sleep(3600)
