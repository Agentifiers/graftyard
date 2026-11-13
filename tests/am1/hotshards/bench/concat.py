#!/usr/bin/env python3
"""concat.py OUT PACK... -> one pack holding every entry of the given packs, in order: each pack's entries are copied
as they are (OFS_DELTA offsets are relative, so they stay right), the object counts are summed, and a new trailer is
written."""
import hashlib, struct, sys

out, packs = sys.argv[1], sys.argv[2:]
bodies, n = [], 0
for p in packs:
    b = open(p, "rb").read()
    assert b[:4] == b"PACK", p
    n += struct.unpack(">I", b[8:12])[0]
    bodies.append(b[12:-20])
h = hashlib.sha1()
with open(out, "wb") as f:
    head = b"PACK" + struct.pack(">II", 2, n)
    for part in [head, *bodies]:
        h.update(part)
        f.write(part)
    f.write(h.digest())
