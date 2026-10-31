#!/usr/bin/env python3
"""M0-WP16 L0-10: generate 10,000 replayed agent commits with provenance, as real git objects.

  python3 wp16/provgen.py --linux /home/user/wp16/alt.git --tip <v7.3> --scratch /home/user/wp16/prov.git \
      --out /home/user/wp16/prov-plan.json

Messages and paths are real: commit k takes the message (with its own trailer block) and the first changed path of
the k-th newest non-merge linux commit before --tip. File contents are synthetic. Each agent session is a task on one
branch, forked from main's tip at its start, with 1-4 commits a push, a draft PR opened after its first push, and its
provenance appended to the message's own trailer block:

  run     Graftyard-run: `Graftyard-Session: https://<host>/<repo>/_graftyard/sessions/<id>`, pushed by the
          `graftyard-run` App's installation token after an AgentTask; pushes its session log under
          refs/graftyard/sessions/<id>. Expected to resolve to the session.
  claude  vendor App: `Claude-Session: https://claude.ai/code/<id>`. Expected to resolve to its PR.
  notes   vendor App: `Co-Authored-By:` only. Resolves to its PR through `AgentMeta`. After each push a human or an
          unconfined App (`notes-bot`, not an `AgentApp`) adds a `refs/notes/ai` note per commit, so `AiNote` rows
          go through the restore. An `ActorKind::Agent` may not update `refs/notes/ai` (decisions/DECISIONS.md,
          "Agents do not write `refs/notes/ai`"): on each session's first push the vendor App also tries, and that
          push is marked `refuse` and must be refused.
  forged  a human pushes a `Graftyard-Session` URL naming a live task of another session. Must stay Claimed, and
          resolve to its PR.

Half the PRs merge by squash onto main (`Merged { pr, commit }`) and their branch is deleted; the squash commits
resolve to their PR, and the agent commits keep resolving through their stored rows. Writes the event list in order, each push with every object it adds (type and raw body), and
the expected resolution of every agent and squash commit.
"""
import argparse, base64, json, random, re, subprocess

ap = argparse.ArgumentParser()
ap.add_argument("--linux", required=True)
ap.add_argument("--tip", required=True)
ap.add_argument("--scratch", required=True)
ap.add_argument("--out", required=True)
ap.add_argument("--commits", type=int, default=10_000)
ap.add_argument("--seed", type=int, default=16)
a = ap.parse_args()
random.seed(a.seed)
HOST, REPO = "graftyard.test", "o/mono"

# Real messages and first paths, newest first.
log = subprocess.run(["git", "-C", a.linux, "log", "--no-merges", f"-n{a.commits + 200}", "--format=%x01%B%x02", "--name-only", a.tip],
                     capture_output=True, check=True).stdout.decode("utf-8", "replace")
real = []
for chunk in log.split("\x01")[1:]:
    msg, _, names = chunk.partition("\x02")
    paths = [p for p in names.split("\n") if p.strip()]
    if paths:
        real.append((msg.rstrip("\n"), paths[0]))
real = real[: a.commits]
assert len(real) == a.commits, len(real)

KINDS = [("run", 0.4), ("claude", 0.3), ("notes", 0.2), ("forged", 0.1)]
sessions, k = [], 0
while k < a.commits:
    n = min(random.randint(5, 15), a.commits - k)
    kind = random.choices([x for x, _ in KINDS], [w for _, w in KINDS])[0]
    sid = f"{len(sessions):05d}-{random.getrandbits(48):012x}"
    sessions.append({"id": sid, "kind": kind, "msgs": real[k:k + n], "n": len(sessions) + 1})
    k += n

stream, mark = [], [0]
def data(b):
    stream.append(f"data {len(b)}\n".encode()); stream.append(b); stream.append(b"\n")
def new_mark():
    mark[0] += 1
    return mark[0]
T0 = 1785542400  # 2026-08-01T00:00Z
clock = [T0]
def commit(ref, frm, msg, files, who="agent <agent@example.com>", notes=None):
    m = new_mark()
    clock[0] += 7
    stream.append(f"commit {ref}\nmark :{m}\nauthor {who} {clock[0]} +0000\ncommitter {who} {clock[0]} +0000\n".encode())
    data(msg.encode())
    if frm:
        stream.append(f"from {frm}\n".encode())
    for path, body in files:
        stream.append(f"M 100644 inline {path}\n".encode()); data(body.encode())
    for target in notes or []:
        stream.append(f"N inline {target}\n".encode()); data(json.dumps({"tool": "claude-code", "commit": target}).encode())
    return m

# main starts with one commit.
main_mark = commit("refs/heads/main", None, "Initial commit\n", [("README", "mono\n")], who="human <h@example.com>")
events = [{"k": "push", "ref": "refs/heads/main", "old": None, "new": f":{main_mark}", "actor": {"t": "user", "user": "h"}, "nots": []}]
expect, forged_tasks = {}, [s for s in sessions if s["kind"] == "run"]

# Interleave: 24 live sessions at a time, round-robin by push.
live, queue, notes_tip = [], list(sessions), None
TOKEN = re.compile(r"^[A-Za-z0-9][A-Za-z0-9-]*[ \t]*:")
def trailer(s, msg):
    # Join the message's own trailer block when its last line is a trailer, as `git commit --trailer` does; otherwise
    # start a new paragraph.
    if not TOKEN.match(msg.splitlines()[-1]):
        msg += "\n"
    url = f"https://{HOST}/{REPO}/_graftyard/sessions/{s['id']}"
    if s["kind"] == "run":
        return f"{msg}\nGraftyard-Session: {url}\n"
    if s["kind"] == "claude":
        return f"{msg}\nClaude-Session: https://claude.ai/code/{s['id']}\n"
    if s["kind"] == "notes":
        return f"{msg}\nCo-Authored-By: Claude <noreply@anthropic.com>\n"
    victim = random.choice(forged_tasks)["id"] if forged_tasks else s["id"]
    return f"{msg}\nGraftyard-Session: https://{HOST}/{REPO}/_graftyard/sessions/{victim}\n"
while queue or live:
    while queue and len(live) < 24:
        s = queue.pop(0)
        s.update(ref=f"refs/heads/{'claude' if s['kind'] == 'run' else 'agent'}/{s['id']}", tip=None, base=f":{main_mark}", left=list(s["msgs"]), marks=[])
        s["app"] = {"run": "graftyard-run", "claude": "claude", "notes": "vendor-x", "forged": None}[s["kind"]]
        if s["kind"] == "run":
            events.append({"k": "task", "id": s["id"], "app": "graftyard-run", "ref": s["ref"]})
        live.append(s)
    s = live.pop(0)
    cut = random.randint(1, 4)
    batch, s["left"] = s["left"][:cut], s["left"][cut:]
    old = s["tip"]
    for msg, path in batch:
        s["tip"] = f":{commit(s['ref'], s['tip'] or s['base'], trailer(s, msg), [(path, f'{s['id']} {len(s['marks'])}\n')])}"
        s["marks"].append(s["tip"])
    actor = {"t": "user", "user": "mallory"} if s["kind"] == "forged" else {"t": "app", "app": s["app"]}
    events.append({"k": "push", "ref": s["ref"], "old": old, "new": s["tip"], "actor": actor, "nots": [old or s["base"]]})
    if s["kind"] == "notes":
        targets = [x for x in s["marks"][-len(batch):]]
        if old is None:
            bad = commit("refs/notes/ai", notes_tip, "Notes added by 'git notes add'\n", [], notes=targets)
            events.append({"k": "push", "ref": "refs/notes/ai", "old": notes_tip, "new": f":{bad}", "actor": actor, "nots": [notes_tip], "refuse": True})
        writer = random.choice([{"t": "user", "user": "h"}, {"t": "app", "app": "notes-bot"}])
        m = commit("refs/notes/ai", notes_tip, "Notes added by 'git notes add'\n", [], notes=targets)
        events.append({"k": "push", "ref": "refs/notes/ai", "old": notes_tip, "new": f":{m}", "actor": writer, "nots": [notes_tip]})
        notes_tip = f":{m}"
    if old is None:
        events.append({"k": "pr", "n": s["n"], "ref": s["ref"], "base": s["base"], "head": s["tip"],
                       "meta": {"app": s["app"], "session": s["id"] if s["kind"] == "run" else None}})
    if s["left"]:
        live.append(s)
        continue
    for x in s["marks"]:
        expect[x] = f"session:{s['id']}" if s["kind"] == "run" else f"pr:{s['n']}"
    if s["kind"] == "run":
        lm = commit(f"refs/graftyard/sessions/{s['id']}", None, f"session {s['id']}\n",
                    [("log.json", json.dumps({"schema": 1, "id": s["id"], "plan": "replayed", "pushes": len(s["marks"])}))], who="graftyard-workspace <w@example.com>")
        events.append({"k": "push", "ref": f"refs/graftyard/sessions/{s['id']}", "old": None, "new": f":{lm}", "actor": {"t": "app", "app": "graftyard-workspace", "task": s["id"]}, "nots": []})
    if random.random() < 0.5:
        files = [(p, f"squash {s['id']} {p}\n") for _, p in s["msgs"]]
        sq = commit("refs/heads/main", f":{main_mark}", f"Squash PR #{s['n']}\n\n{s['msgs'][0][0].splitlines()[0]}\n", files, who="human <h@example.com>")
        events.append({"k": "merge", "n": s["n"], "ref": "refs/heads/main", "old": f":{main_mark}", "new": f":{sq}", "actor": {"t": "user", "user": "h"},
                       "nots": [f":{main_mark}"]})
        events.append({"k": "push", "ref": s["ref"], "old": s["tip"], "new": None, "actor": {"t": "user", "user": "h"}, "objects": []})
        main_mark = sq
        expect[f":{sq}"] = f"pr:{s['n']}"

with open(a.scratch + ".fi", "wb") as f:
    f.write(b"".join(stream) + b"done\n")
subprocess.run(["rm", "-rf", a.scratch], check=True)
subprocess.run(["git", "init", "-q", "--bare", a.scratch], check=True)
with open(a.scratch + ".fi", "rb") as f:
    subprocess.run(["git", "-C", a.scratch, "fast-import", "--quiet", "--done", f"--export-marks={a.scratch}.marks"], stdin=f, check=True)
marks = dict(l.split() for l in open(a.scratch + ".marks"))
oid = lambda x: marks[x] if x and x.startswith(":") else x

cat = subprocess.Popen(["git", "-C", a.scratch, "cat-file", "--batch"], stdin=subprocess.PIPE, stdout=subprocess.PIPE)
def body(o):
    cat.stdin.write(f"{o}\n".encode()); cat.stdin.flush()
    _, t, n = cat.stdout.readline().split()
    b = cat.stdout.read(int(n)); cat.stdout.read(1)
    return t.decode(), base64.b64encode(b).decode()
sent = set()
for e in events:
    for f in ("old", "new", "base", "head"):
        if e.get(f):
            e[f] = oid(e[f])
    if "nots" in e:
        nots = [oid(x) for x in e.pop("nots") if x]
        out = subprocess.run(["git", "-C", a.scratch, "rev-list", "--objects", e["new"], *[f"^{x}" for x in nots]], capture_output=True, check=True, text=True).stdout
        objs = []
        for line in out.splitlines():
            o = line.split()[0]
            if o not in sent:
                if not e.get("refuse"):  # a refused push stores nothing, so its objects are sent again later
                    sent.add(o)
                t, b = body(o)
                objs.append({"oid": o, "type": t, "data": b})
        e["objects"] = objs
expect = {oid(k): v for k, v in expect.items()}
kinds = {}
for s in sessions:
    kinds[s["kind"]] = kinds.get(s["kind"], 0) + len(s["msgs"])
json.dump({"host": HOST, "repo": REPO, "events": events, "expect": expect, "kinds": kinds}, open(a.out, "w"))
print(json.dumps({"events": len(events), "objects": len(sent), "expect": len(expect), "agentCommits": sum(kinds.values()), "kinds": kinds,
                  "sessions": len(sessions), "squashes": sum(1 for e in events if e["k"] == "merge"),
                  "notesPushes": sum(1 for e in events if e.get("ref") == "refs/notes/ai" and not e.get("refuse")),
                  "agentNotesAttempts": sum(1 for e in events if e.get("refuse"))}))
