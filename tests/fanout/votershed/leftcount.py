#!/usr/bin/env python3
"""Objects still under the retire-leak networks in graft-m2b's three buckets, by one listing per key family."""
import collections, hashlib, json, os, re, sys, urllib.request
sys.path.insert(0, 'tests/fanout'); import purge
import boto3
nets = set(range(131107, 131296)) | set(range(327680, 327700)) | {65536, 131072, 131073, 131296, 327700}
acct, tok = os.environ["CLOUDFLARE_ACCOUNT_ID"], os.environ["CLOUDFLARE_API_TOKEN"]
v = json.load(urllib.request.urlopen(urllib.request.Request(
    f"https://api.cloudflare.com/client/v4/accounts/{acct}/tokens/verify", headers={"authorization": f"Bearer {tok}"}), timeout=30))
s3 = boto3.client("s3", endpoint_url=f"https://{acct}.r2.cloudflarestorage.com", region_name="auto",
                  aws_access_key_id=v["result"]["id"], aws_secret_access_key=hashlib.sha256(tok.encode()).hexdigest())
fams = ["pack/", "committed/pack/", "upload/", "compose/", "clone/", "archive-cache/", "blob/", "committed/blob/",
        "archive/", "snapshot/", "inline-idx/"]
by = collections.Counter(); total = 0
for site in purge.SITES:
    b = purge.BUCKET.format(install=purge.INSTALL, site=site)
    for f in fams:
        for page in s3.get_paginator("list_objects_v2").paginate(Bucket=b, Prefix=f):
            for o in page.get("Contents", []):
                total += 1
                rest = o["Key"][len(f):]
                m = re.match(r"(?:repo(?:-meta)?:)?(\d+)/", rest)
                if m and int(m.group(1)) in nets and (f"{f}{rest[:m.end()]}" in purge.prefixes(int(m.group(1)))):
                    by[(site, int(m.group(1)))] += 1
left = collections.Counter()
for (s, n), c in by.items(): left[n] += c
print(json.dumps({"keys_listed": total, "objects_left": sum(by.values()), "nets_with_objects": len(left),
                  "by_site": {s: sum(c for (x, _), c in by.items() if x == s) for s in purge.SITES},
                  "below_131217": sum(c for n, c in left.items() if n < 131217),
                  "top": left.most_common(8)}))
