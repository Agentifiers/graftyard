#!/usr/bin/env python3
"""The hosted vault row's view of the vault bucket, through the R2 REST API under the vault account's token
(`tests/m5/results/m5-wp5-vault.md`, "Hosted vault row"). On this row the runner account stands in for the vault
account. Reads R2_ACCOUNT and R2_TOKEN_VAR (the name of the variable holding the token) from the environment, so the
same script reads the install's export bucket under the install token; the token goes only into a header and is never
printed. Prints statuses, error codes, keys, sizes and etags only.

    r2.py BUCKET snap OUT.json          every key with its size and etag
    r2.py BUCKET get KEY OUT            one object's bytes
    r2.py BUCKET put KEY FILE [--create-only]   PUT (with If-None-Match: * when --create-only)
    r2.py BUCKET delete KEY
    r2.py BUCKET lock-get | lock-clear  the bucket's lock rules; lock-clear PUTs an empty rule list
    r2.py BUCKET empty                  deletes every object (only after lock-clear)
    r2.py BUCKET bucket-delete
"""
import json
import os
import sys
import urllib.error
import urllib.parse
import urllib.request

ACCT = os.environ["R2_ACCOUNT"]
TOKEN = os.environ[os.environ["R2_TOKEN_VAR"]]
API = f"https://api.cloudflare.com/client/v4/accounts/{ACCT}/r2/buckets"


def call(method, path, body=None, headers=None, raw=False):
    h = {"authorization": f"Bearer {TOKEN}", "user-agent": "graftyard-vault-hosted"}
    h.update(headers or {})
    req = urllib.request.Request(API + path, data=body, method=method, headers=h)
    try:
        with urllib.request.urlopen(req, timeout=60) as r:
            data = r.read()
            return r.status, (data if raw else json.loads(data or b"{}"))
    except urllib.error.HTTPError as e:
        data = e.read()
        try:
            return e.code, json.loads(data or b"{}")
        except ValueError:
            return e.code, {"body": data[:200].decode(errors="replace")}


def errs(v):
    return [f"{x.get('code')} {x.get('message')}" for x in (v.get("errors") or [])] if isinstance(v, dict) else []


def obj(key):
    return "/objects/" + urllib.parse.quote(key, safe="")


def keys(bucket):
    out, cursor = {}, None
    while True:
        q = "?per_page=1000" + (f"&cursor={urllib.parse.quote(cursor)}" if cursor else "")
        s, v = call("GET", f"/{bucket}/objects{q}")
        if s != 200:
            sys.exit(f"list: {s} {errs(v)}")
        for o in v["result"]:
            out[o["key"]] = [o.get("size"), o.get("etag")]
        cursor = (v.get("result_info") or {}).get("cursor")
        if not (v.get("result_info") or {}).get("is_truncated") or not cursor:
            return out


def main():
    bucket, op, *rest = sys.argv[1:]
    if op == "snap":
        k = keys(bucket)
        json.dump(k, open(rest[0], "w"), indent=1, sort_keys=True)
        by = {}
        for key in k:
            by[key.split("/")[0]] = by.get(key.split("/")[0], 0) + 1
        print(json.dumps({"objects": len(k), "by_prefix": by}))
    elif op == "get":
        s, v = call("GET", f"/{bucket}{obj(rest[0])}", raw=True)
        if s != 200:
            sys.exit(f"get {rest[0]}: {s} {errs(v)}")
        open(rest[1], "wb").write(v)
        print(json.dumps({"get": rest[0], "status": s, "bytes": len(v)}))
    elif op == "put":
        h = {"if-none-match": "*"} if "--create-only" in rest else {}
        s, v = call("PUT", f"/{bucket}{obj(rest[0])}", open(rest[1], "rb").read(), h)
        print(json.dumps({"put": rest[0], "create_only": bool(h), "status": s, "errors": errs(v)}))
    elif op == "delete":
        s, v = call("DELETE", f"/{bucket}{obj(rest[0])}")
        print(json.dumps({"delete": rest[0], "status": s, "errors": errs(v)}))
    elif op == "lock-get":
        s, v = call("GET", f"/{bucket}/lock")
        rules = (v.get("result") or {}).get("rules") if s == 200 else None
        print(json.dumps({"lock": s, "errors": errs(v), "rules": rules}))
    elif op == "lock-clear":
        s, v = call("PUT", f"/{bucket}/lock", json.dumps({"rules": []}).encode(), {"content-type": "application/json"})
        print(json.dumps({"lock_clear": s, "errors": errs(v)}))
    elif op == "empty":
        k = keys(bucket)
        bad = []
        for key in k:
            for attempt in range(3):  # a TLS handshake timeout stopped the first hosted empty mid-bucket
                try:
                    s, v = call("DELETE", f"/{bucket}{obj(key)}")
                    break
                except urllib.error.URLError as e:
                    s, v = 0, {"errors": [{"code": 0, "message": str(e.reason)}]}
            if s != 200 or errs(v):
                bad.append([key, s, errs(v)])
        print(json.dumps({"emptied": len(k) - len(bad), "refused": bad, "left": len(keys(bucket))}))
    elif op == "bucket-delete":
        s, v = call("DELETE", f"/{bucket}")
        print(json.dumps({"bucket_delete": s, "errors": errs(v)}))
    else:
        sys.exit(__doc__)


if __name__ == "__main__":
    main()
