#!/usr/bin/env python3
"""d1upload.py N TIP (run in a bare repository): N concurrent depth-1 protocol-v2 `git upload-pack --stateless-rpc`
fetches of TIP, read to the end, with wall, summed child CPU, bytes and peak RSS (LM-1 round 24c)."""
import subprocess,sys,time,resource,os
T = sys.argv[2]
def pkt(s): s=s.encode(); return b'%04x'%(len(s)+4)+s
body=pkt("command=fetch\n")+b"0001"+pkt("no-progress\n")+pkt(f"want {T}\n")+pkt("deepen 1\n")+pkt("done\n")+b"0000"
n = int(sys.argv[1]); t=time.time(); r0=resource.getrusage(resource.RUSAGE_CHILDREN)
ps=[subprocess.Popen(["git","upload-pack","--stateless-rpc","."],stdin=subprocess.PIPE,stdout=subprocess.PIPE,env={**os.environ,"GIT_PROTOCOL":"version=2"}) for _ in range(n)]
tot=[0]*n
for p in ps: p.stdin.write(body); p.stdin.close()
for i,p in enumerate(ps):
    while (b:=p.stdout.read(1<<20)): tot[i]+=len(b)
    p.wait()
r1=resource.getrusage(resource.RUSAGE_CHILDREN)
cpu=(r1.ru_utime-r0.ru_utime)+(r1.ru_stime-r0.ru_stime)
print(f"n={n} wall {time.time()-t:.1f}s cpu {cpu:.0f}s ({cpu/n:.1f} CPU-s each) bytes {tot[0]/1e6:.0f} MB maxrss {r1.ru_maxrss/1e6:.2f} GB")
