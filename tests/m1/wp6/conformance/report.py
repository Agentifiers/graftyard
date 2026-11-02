"""Per-test results of a conformance run: git's TAP lines in <out>/<version>-<script>.out joined with the front's
event log, <out>/events.tsv (front/main.go).

Each test gets one result:
  pass       ok
  fail       not ok, and the test used only what a hosted service offers
  n/a        not ok, and while it ran the front saw an Apache fixture it cannot map (`unmapped`: a rewrite, a CGI
             script, an auth realm, a namespace or cookie or header location), a server-side setting a hosted
             repository cannot carry (`server-setting`: a hook or a transfer config other than the host's own
             behavior), or a repository the host cannot hold (`mirror-error`, for example SHA-256)
  skip       git skipped it (a missing prerequisite such as EXPENSIVE or HTTP2, or the whole script)
A test that passed is "pass" even if it touched an unmapped path (it then expected that path to fail). The reason
column gives the first event that made a test n/a.

Usage: python3 report.py <out dir>  (Markdown on stdout; also writes <out>/results.tsv)
"""

import collections
import glob
import os
import re
import sys

out = sys.argv[1]
events = collections.defaultdict(list)
if os.path.exists(os.path.join(out, "events.tsv")):
    for line in open(os.path.join(out, "events.tsv"), encoding="utf-8", errors="replace"):
        f = line.rstrip("\n").split("\t")
        if len(f) >= 8:
            version, script, test, kind, method, uri, status, detail = f[:8]
            events[(version, script, test)].append((kind, method, uri, status, detail))

TAP = re.compile(r"^(ok|not ok) (\d+)(?: -| #) ?(.*)$")
rows = []
for path in sorted(glob.glob(os.path.join(out, "*-t*.out"))):
    version, script = os.path.basename(path)[:-4].split("-", 1)
    text = open(path, encoding="utf-8", errors="replace").read()
    m = re.search(r"^1\.\.0 # SKIP (.*)$", text, re.M)
    if m:
        rows.append((version, script, "-", "(whole script)", "skip", m.group(1)))
        continue
    for line in text.splitlines():
        m = TAP.match(line)
        if not m:
            continue
        ok, n, title = m.group(1), m.group(2), m.group(3)
        evs = events.get((version, script, n), [])
        if ok == "ok" and title.startswith("skip "):
            reason = re.search(r"\((.*)\)\s*$", title)
            rows.append((version, script, n, re.sub(r"^skip | \(.*\)\s*$", "", title), "skip",
                         reason.group(1) if reason else ""))
            continue
        if ok == "ok":
            rows.append((version, script, n, title, "pass", ""))
            continue
        why = ""
        for kind, method, uri, status, detail in evs:
            if kind == "unmapped":
                why = f"Apache fixture /{detail}/ ({method} {uri.split('?')[0]})"
            elif kind == "server-setting":
                why = f"server-side {detail}"
            elif kind == "mirror-error":
                why = "host cannot hold the repository: " + (
                    "SHA-256" if "hash algorithm" in detail else detail[-120:])
            if why:
                break
        rows.append((version, script, n, title, "n/a" if why else "fail", why))

with open(os.path.join(out, "results.tsv"), "w") as f:
    for r in rows:
        f.write("\t".join(r) + "\n")

by = collections.OrderedDict()
for v, s, n, t, res, why in rows:
    by.setdefault((s, v), collections.Counter())[res] += 1
print("| Script | git | run | pass | fail | n/a (hosted) | skip |")
print("| --- | --- | --- | --- | --- | --- | --- |")
for (s, v), c in sorted(by.items()):
    run = c["pass"] + c["fail"] + c["n/a"]
    print(f"| {s} | {v} | {run} | {c['pass']} | {c['fail']} | {c['n/a']} | {c['skip']} |")
tot = collections.Counter()
for c in by.values():
    tot.update(c)
print(f"| total | | {tot['pass'] + tot['fail'] + tot['n/a']} | {tot['pass']} | {tot['fail']} | {tot['n/a']} |"
      f" {tot['skip']} |")
print()
print("## Failures")
print()
for v, s, n, t, res, why in rows:
    if res == "fail":
        print(f"- git {v} {s} #{n}: {t}")
print()
print("## Not applicable to a hosted service")
print()
print("| Script | # | Test | git | Reason |")
print("| --- | --- | --- | --- | --- |")
na = collections.OrderedDict()
for v, s, n, t, res, why in rows:
    if res == "n/a":
        na.setdefault((s, int(n), t), []).append((v, why))
for (s, n, t), vs in sorted(na.items()):
    print(f"| {s} | {n} | {t} | {', '.join(v for v, _ in vs)} | {vs[0][1]} |")
print()
print("## Skipped by git")
print()
sk = collections.Counter((s, why) for v, s, n, t, res, why in rows if res == "skip")
for (s, why), c in sorted(sk.items()):
    print(f"- {s}: {c} ({why})")
