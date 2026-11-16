#!/usr/bin/env python3
"""B0-30 rerun: which config entry 208,160 of repo:1048590 holds. No member route returns an entry's body, so this
rebuilds the canonical bytes (`crates/raft/src/encode.rs`: domain tag, term, index, prev, format, tiers, at,
requests, extend, body) for every Normal and Degraded{lost} candidate over the group's voters, and prints the ones
whose SHA-256 is the stored digest. The term, index, at, prev and digest come from the leader's `/log` (saved in
tests/am1/raw/b030r2-log208160.jsonl). Printed on 2026-11-16: [('Degraded lost 1', 5, 5)], one match of 196."""
import hashlib, struct
prev = bytes.fromhex("7de84e606e17027558fe2b6ee8aa373ba9e69ab8502e5eb0c935e62f7d18d5ef")
target = "7c6d197d197560a5e35e7aba723d5ef20618146f3f243e9a1956d138b26b458e"
u8=lambda v: struct.pack(">B",v); u16=lambda v: struct.pack(">H",v); u32=lambda v: struct.pack(">I",v); u64=lambda v: struct.pack(">Q",v)
s=lambda x: u32(len(x.encode()))+x.encode()
def mem(site,gen,slot=None,kind=0): return u8(site)+u8(kind)+u8(site if slot is None else slot)+u32(gen)+s(f"repo:1048590#{gen}#f{site}")
def plc(f1gen, gen): return u32(3)+mem(0,1)+mem(1,f1gen)+mem(2,1)+u32(0)+u8(0)+u32(gen)
def entry(cfg, extend=None):
    b=b"graftyard/log/1"+u64(153)+u64(208160)+prev+u16(0)+u16(1)+u8(0)+u64(1794816842248)+u32(0)
    b+= u8(0) if extend is None else u8(1)+u64(extend[0])+u64(extend[1])
    return b+u8(1)+cfg
hits=[]
for f1 in range(5,12):
  for gen in range(5,12):
    for name,cfg in [("Normal",u8(0)+plc(f1,gen))]+[(f"Degraded lost {l}",u8(1)+plc(f1,gen)+u8(l)) for l in (0,1,2)]:
      for ext in [None]:
        d=hashlib.sha256(entry(cfg,ext)).hexdigest()
        if d==target: hits.append((name,f1,gen))
print("hits", hits)
# sanity: bytes field 181 is row length, not canonical; print canonical lengths
print(len(entry(u8(1)+plc(5,5)+u8(1))))
