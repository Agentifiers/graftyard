exec(open('tests/fanout/votershed/a1.py').read().split('# open distribution')[0])
cpu={0:{'00:59':0.27,'01:00':0.28,'01:01':0.4,'01:02':0.34,'01:03':0.33,'01:04':0.4,'01:05':0.16},
     2:{'00:59':0.04,'01:00':0.26,'01:01':0.23,'01:02':0.3,'01:03':0.34,'01:04':0.35,'01:05':0.12},
     1:{'00:59':0.01,'01:00':0.06,'01:01':0.07,'01:02':0.08,'01:03':0.09,'01:04':0.08,'01:05':0.0}}
import datetime
for s in sorted(rows):
    c=collections.Counter(datetime.datetime.fromtimestamp(x[0]/1000,datetime.UTC).strftime('%H:%M') for x in ent[s])
    print(s,' '.join(f"{m}: {c.get(m,0)/60:.0f}/s cpu {cpu[s][m]:.2f} -> {1000*cpu[s][m]/max(1,c.get(m,0)/60):.2f} ms/req" for m in sorted(cpu[s])))
