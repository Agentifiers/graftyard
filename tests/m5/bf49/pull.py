#!/usr/bin/env python3
"""BF-49's hosted puller (`build/BUILD-PLAN.md`, M5-WP5): pulls an install's export endpoints the way a vault does and
checks what it gets. The pure half and its reference test are crates/git/src/export.rs and crates/git/tests/export.rs.

    pull.py --edge https://git.example --token-file PATH [--out DIR] [--chains N] [--pack NETWORK:PACK_ID ...]
            [--revoke] [--probe-s 30]

- `GET batches` page by page through `Link: <...>; rel="next"`: every frame's SHA-256, every `ExportObject` decoded,
  each chain's entries contiguous from the chain's first appearance, positions increasing.
- `GET chains/{chain}/entries` for the N chains with the most entries the feed carried, from 1 (or from the first
  index the feed carried, after a 410), page by page: every frame verified, and the rows byte-equal to the feed's over
  the indexes both carry.
- `GET chains/{chain}/snapshots?at=<large>` for up to 3 `repo:<id>` chains: 404 (no snapshot) is accepted; a 200's
  frames are verified as (key, object) pairs, core tier first.
- `GET packs/{pack_id}?network=<id>` for each --pack: `X-Graftyard-Checksum` equals the body's SHA-256.
- --revoke: `DELETE token`, then `GET batches` every 250 ms for --probe-s seconds. PASS iff no 200 for a probe sent
  later than 10,000 ms (READ_CACHE) after the 204 arrived, and at least one 401.

Writes DIR/report.json (default tests/m5/results/raw/bf49) and prints a summary. Exits 0 when every check passes. The
token is read from a file and never printed or written to the report.
"""
import argparse
import hashlib
import json
import os
import re
import struct
import sys
import time
import urllib.error
import urllib.parse
import urllib.request

PREFIX = "/api/graftyard/v1/export"
READ_CACHE_MS = 10_000
ENTRIES_LIMIT = 1000
SNAPSHOT_AT = 10**18
PROBE_EVERY_S = 0.25


class FrameError(ValueError):
    pass


# ---- Decoding (pure) ----

def frame(b):
    """One frame, as crates/git/src/export.rs `frame` writes it: 4-byte big-endian length, bytes, SHA-256."""
    return struct.pack(">I", len(b)) + b + hashlib.sha256(b).digest()


def unframe(b):
    """The frames of a body, each checked against its digest. Raises FrameError naming the bad frame."""
    out, at = [], 0
    while at < len(b):
        if at + 4 > len(b):
            raise FrameError(f"frame {len(out)}: shorter than its length")
        (n,) = struct.unpack(">I", b[at:at + 4])
        body = b[at + 4:at + 4 + n]
        if len(body) != n:
            raise FrameError(f"frame {len(out)}: shorter than its length")
        digest = b[at + 4 + n:at + 4 + n + 32]
        if len(digest) != 32:
            raise FrameError(f"frame {len(out)}: without its digest")
        if hashlib.sha256(body).digest() != digest:
            raise FrameError(f"frame {len(out)} fails its digest")
        out.append(body)
        at += 4 + n + 32
    return out


def decode_object(b):
    """An ExportObject's stored bytes: a JSON header frame, then one frame per entry, chain by chain. Returns
    {"batch": {"minute", "seq", "chains"}, "entries": [(chain, first, [row, ...]), ...]}; raises ValueError."""
    frames = unframe(b)
    if not frames:
        raise ValueError("no header frame")
    try:
        head = json.loads(frames[0])
        batch, heads = head["batch"], head["entries"]
        minute, seq = int(batch["minute"]), int(batch["seq"])
        chains = [(str(c), int(i)) for c, i in batch["chains"]]
        heads = [(str(c), int(f), int(n)) for c, f, n in heads]
    except (ValueError, KeyError, TypeError) as e:
        raise ValueError(f"header: {e!r}") from None
    rows, entries = frames[1:], []
    for c, first, n in heads:
        if len(rows) < n:
            raise ValueError(f"{c}: {len(rows)} of {n} entries")
        entries.append((c, first, rows[:n]))
        rows = rows[n:]
    if rows:
        raise ValueError("frames past the last chain's entries")
    return {"batch": {"minute": minute, "seq": seq, "chains": chains}, "entries": entries}


def position_text(batch):
    return f"{batch['minute']}.{batch['seq']}"


def next_link(header):
    """The URL of a `Link` header's rel="next", or None."""
    for m in re.finditer(r'<([^>]*)>\s*;([^,]*)', header or ""):
        if re.search(r'\brel\s*=\s*"?next"?', m.group(2)):
            return m.group(1)
    return None


class Feed:
    """What the batches pages carried, checked as it arrives."""

    def __init__(self):
        self.last = {}          # chain -> last index the feed carried
        self.first = {}         # chain -> first index the feed carried
        self.rows = {}          # chain -> {index: row bytes}
        self.position = None    # (minute, seq) of the newest object
        self.objects = self.entries = self.bytes = 0
        self.errors = []

    def add(self, obj, raw_len=0):
        """Folds in one decoded ExportObject; appends to self.errors what breaks the feed's rules."""
        b = obj["batch"]
        pos = (b["minute"], b["seq"])
        if self.position is not None and pos <= self.position:
            self.errors.append(f"batch {position_text(b)} is not after {self.position[0]}.{self.position[1]}")
        self.position = pos if self.position is None else max(pos, self.position)
        self.objects += 1
        self.bytes += raw_len
        named = [c for c, _ in b["chains"]]
        if named != [c for c, _, _ in obj["entries"]]:
            self.errors.append(f"batch {position_text(b)}: entries for {[c for c, _, _ in obj['entries']]} where "
                               f"the batch names {named}")
        to = dict(b["chains"])
        for c, first, rows in obj["entries"]:
            prev = self.last.get(c)
            if prev is not None and first != prev + 1:
                kind = "repeats" if first <= prev else "skips"
                self.errors.append(f"batch {position_text(b)}: {c} starts at {first} after {prev} ({kind})")
            if not rows:
                self.errors.append(f"batch {position_text(b)}: {c} carries no entries")
                continue
            if c in to and to[c] != first + len(rows) - 1:
                self.errors.append(f"batch {position_text(b)}: {c} entries {first}..={first + len(rows) - 1} "
                                   f"where the batch names index {to[c]}")
            self.first.setdefault(c, first)
            have = self.rows.setdefault(c, {})
            for k, r in enumerate(rows):
                have.setdefault(first + k, r)
            self.last[c] = first + len(rows) - 1 if prev is None else max(prev, first + len(rows) - 1)
            self.entries += len(rows)

    def counts(self):
        return {"objects": self.objects, "chains": len(self.last), "entries": self.entries, "bytes": self.bytes}


def compare_rows(feed_rows, served_from, served):
    """Indexes where the endpoint's rows (from `served_from`, in order) differ from the feed's, over the overlap."""
    return [served_from + k for k, r in enumerate(served)
            if served_from + k in feed_rows and feed_rows[served_from + k] != r]


def check_snapshot(frames):
    """Errors in a snapshots body's frames: (key, object) pairs of `snapshot/<chain>/<tier>/...` keys, core first."""
    if not frames or len(frames) % 2:
        return [f"{len(frames)} frames: not (key, object) pairs"]
    errors, tiers = [], []
    for k in frames[0::2]:
        try:
            parts = k.decode().split("/")
        except UnicodeDecodeError:
            errors.append("a key that is not text")
            continue
        if len(parts) != 4 or parts[0] != "snapshot":
            errors.append(f"key {k[:80]!r} is not a snapshot key")
            continue
        tiers.append(parts[2])
    if tiers and tiers[0] != "core":
        errors.append(f"tiers {tiers}: core is not first")
    return errors


def judge_revoke(probes):
    """probes: [{"sent_ms", "status", ...}] offset from the 204. PASS iff no 200 sent after READ_CACHE_MS and one 401."""
    ok_after = [p["sent_ms"] for p in probes if p["status"] == 200]
    unauth = [p["sent_ms"] for p in probes if p["status"] == 401]
    last_ok = max(ok_after) if ok_after else None
    first_401 = min(unauth) if unauth else None
    return {
        "last_ok_ms": last_ok,
        "first_401_ms": first_401,
        "n_200": len(ok_after),
        "n_401": len(unauth),
        "n_other": len(probes) - len(ok_after) - len(unauth),
        "pass": (last_ok is None or last_ok <= READ_CACHE_MS) and first_401 is not None,
    }


# ---- HTTP ----

class Client:
    def __init__(self, edge, token, timeout=60):
        self.edge = edge.rstrip("/")
        self._auth = "Bearer " + token
        self.timeout = timeout

    def url(self, path_or_url):
        if path_or_url.startswith(("http://", "https://")):
            return path_or_url
        if path_or_url.startswith("/"):
            return self.edge + path_or_url
        return f"{self.edge}{PREFIX}/{path_or_url}"

    def request(self, path_or_url, method="GET"):
        """(status, headers as a lower-cased dict, body). A transport failure is status 0."""
        req = urllib.request.Request(self.url(path_or_url), method=method,
                                     headers={"Authorization": self._auth, "User-Agent": "graftyard-bf49-pull"})
        try:
            with urllib.request.urlopen(req, timeout=self.timeout) as r:
                return r.status, {k.lower(): v for k, v in r.headers.items()}, r.read()
        except urllib.error.HTTPError as e:
            return e.code, {k.lower(): v for k, v in e.headers.items()}, e.read()
        except (urllib.error.URLError, OSError) as e:
            return 0, {}, repr(getattr(e, "reason", e)).encode()


def query_of(url):
    return dict(urllib.parse.parse_qsl(urllib.parse.urlsplit(url).query))


# ---- The pulls ----

def pull_batches(client, feed):
    """Every batches page through Link next. Returns {"pages", "status", "errors"}."""
    url, pages, seen, errors = "batches", 0, set(), []
    while url:
        if url in seen:
            errors.append(f"Link next repeats {url}")
            break
        seen.add(url)
        status, headers, body = client.request(url)
        if status != 200:
            errors.append(f"batches page {pages}: {status} {body[:200]!r}")
            return {"pages": pages, "status": status, "errors": errors}
        pages += 1
        try:
            objects = unframe(body)
        except FrameError as e:
            errors.append(f"batches page {pages - 1}: {e}")
            break
        for k, raw in enumerate(objects):
            try:
                feed.add(decode_object(raw), len(raw))
            except ValueError as e:
                errors.append(f"batches page {pages - 1} object {k}: {e}")
        url = next_link(headers.get("link"))
        if url:
            after = query_of(url).get("after")
            want = f"{feed.position[0]}.{feed.position[1]}" if feed.position else None
            if after != want:
                errors.append(f"Link next after={after} where the page ended at {want}")
    return {"pages": pages, "status": 200, "errors": errors}


def pull_entries(client, chain, feed):
    """A chain's entries endpoint from 1 (or the feed's first index after a 410), compared with the feed's rows."""
    feed_rows, r = feed.rows.get(chain, {}), {"chain": chain, "errors": []}
    start = 1
    q = urllib.parse.quote(chain, safe=":.")
    url = f"chains/{q}/entries?from={start}&limit={ENTRIES_LIMIT}"
    status, headers, body = client.request(url)
    if status == 410 and feed.first.get(chain, 1) > 1:
        r["gone_below"] = start = feed.first[chain]
        url = f"chains/{q}/entries?from={start}&limit={ENTRIES_LIMIT}"
        status, headers, body = client.request(url)
    r.update(start=start, pages=0, entries=0, compared=0, mismatched=[])
    at = start
    while True:
        if status != 200:
            r["errors"].append(f"from={at}: {status} {body[:200]!r}")
            r["status"] = status
            break
        r["pages"] += 1
        try:
            rows = unframe(body)
        except FrameError as e:
            r["errors"].append(f"from={at}: {e}")
            break
        bad = compare_rows(feed_rows, at, rows)
        r["mismatched"] += bad
        r["compared"] += sum(1 for k in range(len(rows)) if at + k in feed_rows)
        r["entries"] += len(rows)
        at += len(rows)
        nxt = next_link(headers.get("link"))
        if not nxt:
            r["status"] = 200
            break
        if query_of(nxt).get("from") != str(at):
            r["errors"].append(f"Link next from={query_of(nxt).get('from')} after a page ending at {at - 1}")
            break
        status, headers, body = client.request(nxt)
    r["last"] = at - 1
    if r["mismatched"]:
        r["errors"].append(f"{len(r['mismatched'])} rows differ from the feed's, first at {r['mismatched'][0]}")
    if r.get("status") == 200 and chain in feed.last and r["last"] < feed.last[chain]:
        r["errors"].append(f"the endpoint ends at {r['last']} where the feed reached {feed.last[chain]}")
    if r.get("status") == 200 and feed_rows and r["compared"] == 0:
        r["errors"].append("no row overlapped the feed's")
    return r


def pull_snapshot(client, chain):
    status, _, body = client.request(f"chains/{urllib.parse.quote(chain, safe=':.')}/snapshots?at={SNAPSHOT_AT}")
    r = {"chain": chain, "status": status, "errors": []}
    if status == 404:
        return r
    if status != 200:
        r["errors"].append(f"{status} {body[:200]!r}")
        return r
    try:
        frames = unframe(body)
    except FrameError as e:
        r["errors"].append(str(e))
        return r
    r["keys"] = [k.decode(errors="replace") for k in frames[0::2]]
    r["bytes"] = len(body)
    r["errors"] += check_snapshot(frames)
    return r


def pull_pack(client, spec):
    network, _, pack = spec.partition(":")
    r = {"pack": spec, "errors": []}
    if not network or not pack:
        r["errors"].append("--pack is NETWORK:PACK_ID")
        return r
    status, headers, body = client.request(f"packs/{urllib.parse.quote(pack)}?network={urllib.parse.quote(network)}")
    r.update(status=status, bytes=len(body))
    if status != 200:
        r["errors"].append(f"{status} {body[:200]!r}")
        return r
    want, got = headers.get("x-graftyard-checksum"), hashlib.sha256(body).hexdigest()
    r.update(checksum=want, sha256=got)
    if want != got:
        r["errors"].append(f"X-Graftyard-Checksum {want} but the body's SHA-256 is {got}")
    return r


def revoke(client, feed, probe_s):
    sent = time.monotonic()
    status, _, body = client.request("token", method="DELETE")
    t0 = time.monotonic()
    r = {"delete_status": status, "delete_ms": round((t0 - sent) * 1000), "probes": []}
    if status != 204:
        r["errors"] = [f"DELETE token: {status} {body[:200]!r}"]
        r.update(judge_revoke([]))
        return r
    probe = f"batches?after={feed.position[0]}.{feed.position[1]}" if feed.position else "batches"
    tick = t0
    while time.monotonic() - t0 < probe_s:
        s = time.monotonic()
        st, _, _ = client.request(probe)
        e = time.monotonic()
        r["probes"].append({"sent_ms": round((s - t0) * 1000), "done_ms": round((e - t0) * 1000), "status": st})
        tick += PROBE_EVERY_S
        time.sleep(max(0.0, tick - time.monotonic()))
    r.update(judge_revoke(r["probes"]))
    r["errors"] = [] if r["pass"] else [
        f"last 200 at {r['last_ok_ms']} ms, first 401 at {r['first_401_ms']} ms (a 200 is allowed until "
        f"{READ_CACHE_MS} ms and a 401 must follow)"]
    return r


def run(client, chains_n, packs, do_revoke, probe_s):
    feed = Feed()
    report = {"batches": pull_batches(client, feed)}
    report["batches"]["errors"] += feed.errors
    report["feed"] = feed.counts()
    report["feed"]["position"] = f"{feed.position[0]}.{feed.position[1]}" if feed.position else None
    report["feed"]["per_chain"] = {c: [feed.first[c], feed.last[c]] for c in sorted(feed.last)}
    top = sorted(feed.last, key=lambda c: (-len(feed.rows[c]), c))[:chains_n]
    report["entries"] = [pull_entries(client, c, feed) for c in top]
    repos = sorted((c for c in feed.last if re.fullmatch(r"repo:\d+", c)), key=lambda c: int(c[5:]))[:3]
    report["snapshots"] = [pull_snapshot(client, c) for c in repos]
    report["packs"] = [pull_pack(client, p) for p in packs]
    if do_revoke:
        report["revoke"] = revoke(client, feed, probe_s)
    checks = [("batches", not report["batches"]["errors"] and feed.objects > 0)]
    checks += [(f"entries {e['chain']}", not e["errors"]) for e in report["entries"]]
    checks += [(f"snapshots {s['chain']} ({s['status']})", not s["errors"]) for s in report["snapshots"]]
    checks += [(f"pack {p['pack']}", not p["errors"]) for p in report["packs"]]
    if do_revoke:
        checks.append(("revoke", report["revoke"]["pass"]))
    report["checks"] = [{"name": n, "pass": bool(ok)} for n, ok in checks]
    report["pass"] = all(ok for _, ok in checks)
    return report


def summary(report):
    f = report["feed"]
    lines = [f"batches: {report['batches']['pages']} pages, {f['objects']} objects, {f['chains']} chains, "
             f"{f['entries']} entries, {f['bytes']} bytes, last position {f['position']}"]
    for e in report["entries"]:
        lines.append(f"entries {e['chain']}: from {e.get('start')} to {e.get('last')}, {e.get('pages')} pages, "
                     f"{e.get('compared')} compared with the feed")
    for s in report["snapshots"]:
        lines.append(f"snapshots {s['chain']}: {s['status']}" + (f" {s['keys']}" if s.get("keys") else ""))
    for p in report["packs"]:
        lines.append(f"pack {p['pack']}: {p.get('status')} {p.get('bytes')} bytes")
    if "revoke" in report:
        v = report["revoke"]
        lines.append(f"revoke: DELETE {v['delete_status']}; {len(v['probes'])} probes: {v['n_200']}x200 "
                     f"{v['n_401']}x401 {v['n_other']} other; last_ok_ms {v['last_ok_ms']} "
                     f"first_401_ms {v['first_401_ms']}")
    for part in ("batches", "entries", "snapshots", "packs", "revoke"):
        items = report.get(part)
        for item in ([items] if isinstance(items, dict) else items or []):
            for err in item.get("errors", []):
                lines.append(f"  error ({part}): {err}")
    for c in report["checks"]:
        lines.append(f"{'PASS' if c['pass'] else 'FAIL'} {c['name']}")
    lines.append("BF-49: " + ("PASS" if report["pass"] else "FAIL"))
    return "\n".join(lines)


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--edge", required=True, help="the edge's base URL, such as https://git.example")
    ap.add_argument("--token-file", required=True, help="the export token `graftyard vault token` wrote")
    ap.add_argument("--out", default="tests/m5/results/raw/bf49", help="the folder report.json goes in")
    ap.add_argument("--chains", type=int, default=5, help="how many chains to pull through the entries endpoint")
    ap.add_argument("--pack", action="append", default=[], metavar="NETWORK:PACK_ID", help="a pack to fetch")
    ap.add_argument("--revoke", action="store_true", help="revoke the token and probe until it is refused")
    ap.add_argument("--probe-s", type=float, default=30.0)
    a = ap.parse_args(argv)
    with open(a.token_file, encoding="utf-8") as f:
        token = f.read().strip()
    if not token or any(c.isspace() for c in token):
        print("pull.py: the token file does not hold one token", file=sys.stderr)
        return 2
    report = run(Client(a.edge, token), a.chains, a.pack, a.revoke, a.probe_s)
    report["edge"] = a.edge
    report["at"] = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
    os.makedirs(a.out, exist_ok=True)
    path = os.path.join(a.out, "report.json")
    with open(path, "w", encoding="utf-8") as f:
        json.dump(report, f, indent=1)
        f.write("\n")
    print(summary(report))
    print(f"wrote {path}")
    return 0 if report["pass"] else 1


if __name__ == "__main__":
    sys.exit(main())
