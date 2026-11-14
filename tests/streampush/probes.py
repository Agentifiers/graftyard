#!/usr/bin/env python3
"""stream-push's hosted refusal-path byte diff: the same receive-pack bodies posted to the edge before and after a
deploy, each reply kept as {status, kept headers, body}. Every body here is one the old edge answered without
committing anything, so a run changes no ref. Run from the worktree root with tests/am1/lf13/env-m3f.sh sourced:

    python3 tests/streampush/probes.py run OUT.json     # one pass over the probes
    python3 tests/streampush/probes.py diff A.json B.json

The admin token comes from $M3_STATE/m3-tokens.txt and the bed token from $BED_TOKEN_FILE; neither reaches argv or
the output.
"""
import base64, gzip, json, os, sys, urllib.error, urllib.request

EDGE = os.environ["GRAFT_EDGE"]
TOK = dict(l.split(" ", 1) for l in open(os.path.join(os.environ["M3_STATE"], "m3-tokens.txt")).read().split("\n") if l)
ADMIN = TOK["admin"].strip()
BED = open(os.environ["BED_TOKEN_FILE"]).read().strip()
REPO = "m3org/stream-push"
KEEP = {"content-type", "cache-control", "expires", "pragma", "retry-after", "www-authenticate",
        "x-github-enterprise-version", "content-encoding"}
Z, A = "0" * 40, "a" * 40


def pkt(s):
    b = s if isinstance(s, bytes) else s.encode()
    return b"%04x" % (len(b) + 4) + b


def cmds(lines, caps="report-status side-band-64k agent=git/2.47.0"):
    out = b""
    for i, l in enumerate(lines):
        out += pkt((l + ("\0" + caps if i == 0 else "") + "\n").encode())
    return out + b"0000"


def pack(n):
    # Not a git pack: every probe below is answered before validate reads it, or by validate's 422.
    return b"PACK" + bytes((i * 31) % 251 for i in range(n - 4))


ONE = [f"{Z} {A} refs/heads/sp-probe"]
PROBES = [
    ("bare flush (probe_rpc)", b"0000", {}),
    ("empty body", b"", {}),
    ("bad command", cmds([f"{Z} {A} HEAD"]) + pack(100), {}),
    ("bad command, 3 MB", cmds([f"{Z} {A} HEAD"]) + pack(3 << 20), {}),
    ("no flush", pkt(ONE[0] + "\0report-status\n"), {}),
    ("shallow", pkt(f"shallow {A}\n") + b"0000", {}),
    ("no pack", cmds(ONE), {}),
    ("too many atomic refs", cmds([f"{Z} {A} refs/heads/a{i}" for i in range(10_001)],
                                 "report-status side-band-64k atomic"), {}),
    ("not a pack, 2 KB (validate 422)", cmds(ONE) + pack(2048), {}),
    ("not a pack, 3 MB (validate 422)", cmds(ONE) + pack(3 << 20), {}),
    ("over the cap, 81 MB", cmds(ONE) + pack(81_000_000), {}),
    ("over the cap, report-status-v2 no side-band", cmds(ONE, "report-status-v2") + pack(81_000_000), {}),
    ("gzip, not a pack", gzip.compress(cmds(ONE) + pack(4096)), {"content-encoding": "gzip"}),
    ("gzip, corrupt", b"\x1f\x8bnot gzip", {"content-encoding": "gzip"}),
    ("placement failing, 3 MB", cmds(ONE) + pack(3 << 20), {"x-graft-place-fail": "1", "x-bed-token": BED}),
    ("placement failing, 81 MB", cmds(ONE) + pack(81_000_000), {"x-graft-place-fail": "1", "x-bed-token": BED}),
    ("anonymous", cmds(ONE) + pack(100), {"authorization": None}),
]


def call(body, extra):
    h = {"content-type": "application/x-git-receive-pack-request", "user-agent": "git/2.47.0",
         "authorization": "Basic " + base64.b64encode(f"x:{ADMIN}".encode()).decode()}
    for k, v in extra.items():
        if v is None:
            h.pop(k, None)
        else:
            h[k] = v
    req = urllib.request.Request(f"{EDGE}/{REPO}.git/git-receive-pack", data=body, headers=h, method="POST")
    try:
        r = urllib.request.urlopen(req, timeout=300)
        status, hs, b = r.status, r.headers, r.read()
    except urllib.error.HTTPError as e:
        status, hs, b = e.code, e.headers, e.read()
    except Exception as e:  # a reset or timeout is an answer too
        return {"status": 0, "headers": {}, "body": str(e)[:200]}
    text = b.decode("utf-8", "backslashreplace")
    for s in (ADMIN, BED):
        assert s not in text
    return {"status": status, "headers": {k.lower(): v for k, v in hs.items() if k.lower() in KEEP}, "body": text}


def main():
    if sys.argv[1] == "run":
        out = {}
        for name, body, extra in PROBES:
            out[name] = call(body, extra)
            print(f"{out[name]['status']:4}  {name}  {out[name]['body'][:90]!r}", flush=True)
        json.dump(out, open(sys.argv[2], "w"), indent=1, sort_keys=True)
    else:
        a, b = (json.load(open(p)) for p in sys.argv[2:4])
        bad = [k for k in a if a[k] != b.get(k)]
        for k in bad:
            print(f"DIFF {k}\n  A {a[k]}\n  B {b.get(k)}")
        print(f"{len(a) - len(bad)} of {len(a)} identical")
        sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
