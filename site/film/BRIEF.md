# Graftyard push loop

Spec: 1350x1080@30, 12s, format: loop, audio: none (muted feature loop, `render.sh --loop --no-audio`)

Graftyard is a git code host that GitHub clients treat as GitHub Enterprise Server 3.22, built on Cloudflare. Each
repository is a Raft group with one member on each of three sites in three regions: North America (enam), Europe
(weur) and Asia-Pacific (apac). A push is acknowledged only after its entry is held on two sites in two regions, two
continents. Measured commit p50 at one entry in flight: 172 to 370 ms (p99 399 to 690 ms); one cross-region round is
p50 107 to 189 ms (design/DESIGN.md, "Scale"). The loop shows a commit, so it shows 214 ms.

Supplied: the brand (ground #0f2a1d, ink #f6f7f3, muted #97a39a, accent #9cc253), Geist and Geist Mono, and the
branch-and-leaf mark (site/public/assets/graftyard-mark.svg). The loop plays muted in a 5:4 frame on the page's dark
band, so its ground is the band's colour.

Brand-supplied looks (lint-film cinetic:brand-supplied): green as the ground and the accent.

Bans: no invented customers or figures; no drawn stand-in for the mark.
