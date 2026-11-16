# F76 trace: BF-54 run, graft-m4a, 2026-11-15

Queried 2026-11-15 after the run with the scripts in this directory (`source tests/am1/lf13/env-m4a.sh`).

## Broker on site a: poll outcomes per minute (cnt.mjs ... broker/poll $workers.outcome)
```
21:04 ok=1375
21:05 ok=1589
21:06 ok=1642
21:07 ok=1658
21:08 ok=1776
21:09 ok=545 exceededMemory=988 canceled=8
21:10 ok=121 exceededMemory=1836 canceled=151
21:11 ok=124 exceededMemory=882 canceled=33
21:12 ok=1300 exceededMemory=642 canceled=454
21:13 ok=588 exceededMemory=671 canceled=726
21:14 ok=1157 exceededMemory=618 canceled=152
21:15 ok=348 exceededMemory=1638 canceled=52
```

## exceededMemory by second, each site (sec.mjs)
```
site a: 09:00:988 09:44:1836 11:12:882 12:40:1313 13:24:618 14:52:940 15:36:698 16:20:953 17:04:999 20:44:877 22:12:975 22:56:991 24:24:872 33:56:970
site b: 10:28:757 11:56:924 15:36:904 17:04:998 20:44:956
site c: 10:28:993 16:20:951
```

## The broker object: CPU and peak memory per minute (docpu.py)
```
broker:actions:7100001.11 20:54:0.7s/33M 20:55:3.0s/26M 20:56:6.6s/35M 20:57:10.0s/35M 20:58:14.1s/47M 20:59:15.9s/61M 21:00:16.6s/55M 21:01:17.2s/56M 21:02:16.1s/109M 21:03:15.3s/58M 21:04:16.2s/80M 21:05:17.0s/58M 21:06:15.0s/98M 21:07:15.9s/83M 21:08:11.8s/219M 21:09:17.4s/263M 21:10:22.4s/262M 21:11:23.8s/270M 21:12:10.9s/258M 21:13:29.1s/265M 21:14:5.1s/200M 21:15:23.9s/266M
```

## The shard's members: CPU and peak memory per minute
```
actions:7100001.11#1#f0 21:00:20.7s/52M 21:01:18.9s/48M 21:02:19.1s/50M 21:03:16.8s/50M 21:04:17.9s/47M 21:05:17.1s/43M 21:06:17.0s/49M 21:07:16.7s/52M 21:08:18.5s/65M 21:09:14.2s/45M 21:10:9.1s/44M 21:11:13.2s/65M 21:12:16.4s/65M 21:13:15.0s/65M 21:14:14.1s/62M 21:15:6.8s/50M
actions:7100001.11#1#f1 21:00:0.2s/12M 21:01:0.1s/13M 21:02:0.1s/13M 21:09:1.5s/15M 21:10:2.1s/16M 21:11:2.1s/17M 21:12:0.9s/15M 21:13:0.7s/14M 21:14:1.5s/16M 21:15:1.6s/12M
actions:7100001.11#1#f2 21:00:1.9s/19M 21:01:1.5s/18M 21:02:1.1s/19M 21:03:0.8s/20M 21:04:1.0s/20M 21:05:0.8s/19M 21:06:0.9s/20M 21:07:0.7s/20M 21:08:1.9s/36M 21:09:2.6s/26M 21:10:5.3s/32M 21:11:2.2s/36M 21:12:2.1s/29M 21:13:1.7s/36M 21:14:1.8s/35M 21:15:4.6s/34M
actions:7100001.11#2#f1 21:00:4.7s/22M 21:01:3.4s/20M 21:02:2.5s/20M 21:03:2.0s/21M 21:04:2.3s/21M 21:05:2.2s/21M 21:06:2.4s/22M 21:07:2.5s/21M 21:08:4.1s/20M 21:09:11.9s/47M 21:10:10.0s/51M 21:11:6.0s/42M 21:12:4.0s/42M 21:13:4.1s/34M 21:14:5.3s/46M 21:15:10.0s/50M
```

## The edge's broker/message: per minute, count and wall p50/p99/max (ms), and polls over 100 s (edge.mjs)
```
21:04 {"n":4289,"p50":50329,"p99":53056,"max":54271} over100s 0
21:05 {"n":4277,"p50":50342,"p99":52359,"max":52733} over100s 0
21:06 {"n":4340,"p50":50373,"p99":52287,"max":53085} over100s 0
21:07 {"n":4138,"p50":50272,"p99":52698,"max":53248} over100s 0
21:08 {"n":4297,"p50":50376,"p99":52326,"max":52753} over100s 0
21:09 {"n":2113,"p50":4235,"p99":68663,"max":99997} over100s 0
21:10 {"n":1859,"p50":87907,"p99":100007,"max":100996} over100s 84
21:11 {"n":1846,"p50":96322,"p99":100011,"max":100138} over100s 137
21:12 {"n":3713,"p50":4721,"p99":100007,"max":100081} over100s 167
21:13 {"n":2123,"p50":1251,"p99":100034,"max":100153} over100s 346
21:14 {"n":3589,"p50":50785,"p99":100000,"max":100006} over100s 19
21:15 {"n":1786,"p50":21039,"p99":100002,"max":100233} over100s 34
```

## Site a: per minute, broker/poll and member/runsvc count/p50/p99 (ms) (tl.mjs; counts in this view are sampled, use cnt.mjs for exact counts)
```
21:04 | a: poll 1295/50191/51200 runsvc 10138/1/273/411 | b: poll 0/0/0 runsvc 0/0/0/0 | c: poll 0/0/0 runsvc 0/0/0/0
21:05 | a: poll 1432/50119/51131 runsvc 11169/1/72/702 | b: poll 0/0/0 runsvc 0/0/0/0 | c: poll 0/0/0 runsvc 0/0/0/0
21:06 | a: poll 1460/50155/51172 runsvc 10690/1/249/364 | b: poll 0/0/0 runsvc 0/0/0/0 | c: poll 0/0/0 runsvc 0/0/0/0
21:07 | a: poll 1510/49875/50877 runsvc 8740/1/251/299 | b: poll 0/0/0 runsvc 0/0/0/0 | c: poll 0/0/0 runsvc 0/0/0/0
21:08 | a: poll 1604/50061/51243 runsvc 9326/1/56/325 | b: poll 0/0/0 runsvc 0/0/0/0 | c: poll 0/0/0 runsvc 0/0/0/0
21:09 | a: poll 1410/46349/66546 runsvc 11211/766/2062/2613 | b: poll 0/0/0 runsvc 2324/2/164/3970 | c: poll 0/0/0 runsvc 1824/302/2089/2327
21:10 | a: poll 2085/25629/34928 runsvc 4502/490/1838/2271 | b: poll 40/15450/31235 runsvc 3131/22/3486/3789 | c: poll 15/8353/10814 runsvc 5156/366/3966/4589
21:11 | a: poll 1039/21951/27883 runsvc 5517/381/1551/2643 | b: poll 834/39247/47662 runsvc 3863/2/290/2074 | c: poll 993/22591/24253 runsvc 754/12/188/204
```

## member/runsvc exact count per minute on site a
```
20:58 ok=17425
20:59 ok=23510
21:00 ok=26050
21:01 ok=25705
21:02 ok=24053
21:03 ok=18671
21:04 ok=21287
21:05 ok=24563
21:06 ok=24538
21:07 ok=20863
21:08 ok=20831
21:09 ok=12368
21:10 ok=4886
21:11 ok=6442
```
