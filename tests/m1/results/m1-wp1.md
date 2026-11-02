# M1-WP1: site scripts and placement (first slice)

Run 2026-10-31 in the Milestone 0 account. The real Rust site script (`workers/site`, built with `worker-build` 0.8.7)
was deployed as three sites, `graft-m1-site-a` (enam), `-b` (weur), and `-c` (apac), by `workers/site/deploy.mjs`
from `tests/m1/wp1/sites.json`. Each script binds every site's `Member` and `Registry` classes with `script_name`. The
driver `tests/m1/wp1/driver.ts` (`graft-m1-driver`) joins a chain's three bootstrap members and reads back each site's
registry row.

## What ran

`POST /join?chain=repo:42&gen=1` on the driver, first call (each member measures 20 binding round trips to each peer):

| Member | Peer | Median RTT |
| --- | --- | --- |
| `repo:42#1#f0` (enam) | weur | 102 ms |
| `repo:42#1#f0` (enam) | apac | 194 ms |
| `repo:42#1#f1` (weur) | enam | 98 ms |
| `repo:42#1#f1` (weur) | apac | 187 ms |
| `repo:42#1#f2` (apac) | enam | 201 ms |
| `repo:42#1#f2` (apac) | weur | 186 ms |

- Every pair is above `MIN_PAIR_RTT` (30 ms), so `Placement::new` accepts `Tri` on these rows.
- Each site's `MemberRegistry` shard holds `{"chain":"repo:42","do_name":"repo:42#1#f<slot>","generation":1,"sites":[0,1,2]}`.
- A second join the same day took 474 ms against 10,569 ms for the first, and every `Rtt` row kept its first `at`,
  so a member does not remeasure within a day.
- A member given site B's identity on site A's class refused with 409.
- The site script's `workers.dev` URL returns 404: a site script has no public route.

## Not built yet in M1-WP1

The group lookup rule needs a group that answers with its config, which is M1-WP2. The placement-failure retry with a
new generation, the daily recheck inside a served request, and the CLI (`graftyard init`, `sites add`,
`sites add-spare`, `placement set`) are still to do.
