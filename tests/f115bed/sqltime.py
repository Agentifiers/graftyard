#!/usr/bin/env python3
"""F115's SQLite half (`tests/f115bed/run.sh`): times a leader member's own statements on a copy of its SQLite file,
cut down to KEEP stored entries (the rows past it deleted, then VACUUM), so the same file reads at each log length.
Per statement, in µs, over N runs: a `log` row by its index (`stored_entry`, `/entries`), the eight-row length page that
`log_after` starts with, a `request` row by its request id (`outcome_by_request`, a re-send's lookup), and one apply's
transaction as `host::SqlStore` writes it (retention delete, log insert, request insert, raft state), with
`synchronous=OFF`, so it reads the statements' cost and not the local disk's fsync. Reads pick their rows at random
over the whole table.
  python3 tests/f115bed/sqltime.py LEADER.sqlite N [KEEP]"""
import os, random, shutil, sqlite3, sys, time

src, n = sys.argv[1], int(sys.argv[2])
keep = int(sys.argv[3]) if len(sys.argv) > 3 else None
dst = src + ".f115"
shutil.copy(src, dst)
if os.path.exists(src + "-wal"):
    shutil.copy(src + "-wal", dst + "-wal")
c = sqlite3.connect(dst)
if keep is not None:
    lo = c.execute("select min(idx) from log").fetchone()[0]
    c.execute("DELETE FROM log WHERE idx >= ?", (lo + keep,))
    c.execute("DELETE FROM request WHERE idx >= ?", (lo + keep,))
    c.commit()
    c.execute("VACUUM")
c.execute("PRAGMA synchronous=OFF")
lo, hi = c.execute("select min(idx), max(idx) from log").fetchone()
rids = [r[0] for r in c.execute("select rid from request")]
random.seed(1)


def per(f, k):
    s = time.perf_counter()
    for _ in range(k):
        f()
    return round((time.perf_counter() - s) * 1e6 / k, 1)


out = {"stored": hi - lo + 1, "requestRows": len(rids), "fileMB": round(os.path.getsize(dst) / 1e6, 1)}
out["logPointUs"] = per(lambda: c.execute("SELECT row FROM log WHERE idx = ?", (random.randint(lo, hi),)).fetchall(), n)
out["logLensPageUs"] = per(lambda: c.execute(
    "SELECT idx, length(row) FROM log WHERE idx > ? ORDER BY idx LIMIT 8", (random.randint(lo, hi),)).fetchall(), n)
out["requestPointUs"] = per(lambda: c.execute(
    "SELECT idx, at, events FROM request WHERE rid = ?", (random.choice(rids),)).fetchall(), n)
row, ev, nxt = os.urandom(4200), os.urandom(4100), [hi + 1]


def apply():
    i = nxt[0]
    nxt[0] += 1
    with c:
        c.execute("DELETE FROM request WHERE at < ?", (0,))
        c.execute("INSERT OR REPLACE INTO log (idx, term, digest, row) VALUES (?, ?, ?, ?)", (i, 2, b"d" * 32, row))
        c.execute("INSERT INTO request (rid, idx, at, events) VALUES (?, ?, ?, ?) ON CONFLICT (rid) DO NOTHING",
                  (f"zz{i:024d}", i, 1, ev))
        c.execute("INSERT OR REPLACE INTO raft_state (id, row) VALUES (1, ?)", (b"s" * 200,))


out["applyTxUs"] = per(apply, n)
print(out)
c.close()
for ext in ("", "-wal", "-shm"):
    if os.path.exists(dst + ext):
        os.remove(dst + ext)
