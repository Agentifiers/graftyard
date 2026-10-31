# B0-15 cost estimate (2026-10-27)

From the account's own Durable Object usage for the B0-12 sample (1,293 members, Cloudflare GraphQL analytics), scaled to 100,000 groups (300,000 members) that each join and measure their two peers once. Prices are the Workers Paid rates on https://developers.cloudflare.com/durable-objects/platform/pricing/, read on 2026-10-27.

| Per member, measured | Value |
| --- | --- |
| Requests | 34 |
| Active time | 6.5 s |
| Rows written | 17 |
| SQLite storage | about 25 KB |

| Item | Volume | Included | Cost |
| --- | --- | --- | --- |
| Requests | 12.6M | 1M | $1.80 |
| Duration | about 244,000 GB-s | 400,000 GB-s | $0 |
| Rows written | about 5.1M | 50M | $0 |
| Storage | about 7.4 GB, deleted after the run | 5 GB-month | $0 for a day or a week; about $0.50 if kept a month |
| The idle window, if the row passes | 0 | | $0 |

Total: about $2 above the $5 Workers Paid minimum, whether the window is a day or a week.

If idle members did bill duration, 300,000 × 128 MB × 86,400 s is about 3.3 billion GB-s a day, about $41,000. Nothing in the harness wakes an idle member, and `run/b0-15.ts check` reads billed duration by class during the window, so a nonzero reading stops the run early.
