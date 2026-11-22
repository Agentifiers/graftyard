# b034-tail: what still sets B0-34's j9 hits tail after `colo-gate`, and two levers ($0, nothing built)

The orchestrator's ruling on mt15 (2026-11-22) asked for a proposal, with a prediction from mt15's trace, for the two
causes that still set the hits tail:
- (a) the cold edge's first second;
- (b) `chain_open` on a new head version.

No row runs until it is approved. Every number below is from mt15's samples (`tests/am1/raw/a1-samples-b0-34-mwr1wmg9.jsonl.gz`,
run `mwr1wmg9`, the colo arm `read:cloneTtfb`). The counterfactual is `tests/fanout/tailcf.py`, whose default input is
that file.

**Where the samples live.** `tests/am1/raw/` is gitignored under the raw-samples policy (`tests/am1/.gitignore`), so
the file is not in the repository. It exists only in the orb of thread T-01a1ccdd-6620-71fb-83ab-1b0ad470eb03, at
`.worktrees/mt15/tests/am1/raw/a1-samples-b0-34-mwr1wmg9.jsonl.gz` under its repository root. A reviewer without it
cannot rerun `tailcf.py`; the tables below are its printed output.

**The model.** A hit's wall is client + front + max(gate, `chain_open`) + rest:
- `gate` and `chain_open` run at once (`hit-gate`);
- `rest` is what is left of the edge's wall;
- `client` (the client's wall less the edge's) is kept as measured.

On every j9 hit this reproduces the measured total (p99 2,189 ms). The p99 is clonelines' rule: index int(0.99 n) of
157 hits, so the fourth slowest sets it, and measured j9 passes only with at most 3 hits over 1 s.

**Short answer.**
- **Neither lever alone brings measured j9 under 1.1 s:**
  - (a) alone: 1,279 to 1,415 ms;
  - (b) alone: 2,189 ms, since it changes only the hits after the first second.
- **Together they do:** a predicted 1,027 to 1,075 ms. That is with (a) as "start the gate beside the token read", and
  (b) as carrying a set forward (≤ 550 ms) or a head that names every file (≤ 200 ms).
- **A third lever helps only a real deploy:** keep the shared gate entry warm across an edge swap. On a real deploy
  with (b) it gives 773 to 1,060 ms. In B0-34's harness it cannot help, because the row's edge is new and the
  monorepo is new to it (see (a)).
- With outages removed, (b) alone gives 746 to 1,060 ms.

## (a) The cold edge's first second

The first four j9 probes started 8–93 ms after t0, on an edge deployed 50 minutes earlier and idle since. Each was a
manifest hit:

| Start after t0 | Total | `front` | `gate` (member read) | `gate_match` | `chain_open` |
|---|---|---|---|---|---|
| 8 ms | 2,189 | 1,191 | 856 | 2 (no entry) | 87 |
| 37 ms | 2,191 | 1,164 | 870 | 8 (no entry) | 333 |
| 40 ms | 1,842 | 1,216 | 142 | 13 (no entry) | 286 |
| 93 ms | 2,115 | 905 | 1,031 | 25 (no entry) | 449 |

The next probe (an off-arm hit at 929 ms) had `front` 608 ms. The first colo-arm hit after it (2,082 ms) had `front`
0 and `gate` 0, served from an isolate entry 1.48 s old.

**The front against the gate.** Front is about 55% of each cold hit's edge wall, and the gate about 40%:
- front 905–1,216 ms (p50 1,178);
- gate 142–1,031 ms (p50 863).

Both start on a cold isolate, and they run in turn: the gate starts only once `front` (`resolve` with `authenticate`)
has answered.

Why the front is about 1.2 s cold and 250 ms warm (`front > 0` after the first 1.1 s: p50 252, p90 295 ms):
- `resolve` with no `rcache` entries reads three things in turn: the login's account, the account's `Placement`, and
  the name in the owner's `Authz` (`chained` with no guess, `workers/edge/src/lib.rs`).
- The token read (`authenticate`) runs beside it.
- Each read is one member read-index call, about 300–400 ms on a cold isolate [inference: no per-read stage was
  timed on these four; the total over three reads in turn fits].

So the cold front is three reads in turn, where a warm isolate's guess (`rcache::last`) runs them at once.

**Does a real deploy show the same first second?** Partly:
- A new edge version starts with empty isolates, and `rcache` is per isolate, so the first request each new isolate
  takes for an owner pays the cold front.
- Under steady real traffic that is a small share of requests, and in a busy colo many isolates warm within seconds.
  [inference]
- Load34's first second is unusual in three ways:
  - every client's first probe lands within 100 ms on the same owner and repository, so the first four hits all pay
    it at once;
  - the edge has no traffic for that owner before t0, so no isolate has an entry;
  - the colo's shared gate entry cannot exist yet, because nothing has read the repository on this edge (the
    monorepo `-f1` is new this row).
- A real deploy keeps the data center's Cache API contents across a script version change, because the cache is
  per colo, not per version [inference: the cache docs describe the cache by data center and URL, not by version].
  So shared gate entries under 9 s old survive the swap, and a real deploy pays only the front.

**Fixes off the request path, and what each predicts:**

| Lever | What changes | j9 measured p99 | over 1.1 s |
|---|---|---|---|
| none (model) | | 2,189 | 6 |
| `warm`: the edge day warms the shared gate entry after the swap | a cold hit's gate is a 10 ms match | 1,654 | 6 |
| `links`: login and placement outlive `READ_CACHE` as guesses | a cold front is one read (÷3) | 1,415 | 5 |
| `beside`: `links`, and the gate's reads start on the guessed repository with the token read | front + gate becomes max(front ÷ 3, gate) | 1,279 | 3 |
| `links` + `warm` (a real deploy whose shared entry survived) | both | 1,279 | 2 |

1. **Warming the shared entry after the swap does not work for B0-34, and should not be built for it.**
   - A gate entry is keyed by the credential's hash, the repository and the record. The edge day cannot hold a
     client's token: Load34's PAT is the row's.
   - The warm would have to read the gate under the client's credential, from the colo the client will land in
     (LAX), within 9 s of the client's first probe, because the entry lapses at `READ_CACHE` less `COLO_SKEW`.
   - It cannot make an entry older than its fold read, so the revocation bound holds. But a warm that ran at the
     edge day's end is 50 minutes stale by t0. It would have to become a harness step that `a1.mjs` runs seconds
     before t0. That is the harness hiding the cold edge, not a fix.
   - On a real deploy it is unneeded: the shared entries survive the swap.
2. **Guessed links (`links`).**
   - `resolve` already guesses from `rcache::last`, which holds entries at any age. A cold isolate has none, so the
     guess has nothing to use.
   - The only off-path way to give a new isolate a guess is the data center's Cache API: put the login's account and
     the account's `Placement` record there, as guesses only, and the third read as the shared gate entry already is.
   - A guess is never an answer: `chained` keeps a guessed read only when the fresh reads say the same, so nothing
     about revocation changes. A wrong guess costs one read again, as today.
   - Saved: the first two reads' wall on a cold isolate.
   - Cost: a match per cold request, about 6 ms per the probe.
3. **Starting the gate beside the front (`beside`, built on `links`).**
   - A cold isolate's `rcache::last` is empty, so the guessed repository must come from the data center too: `links`
     puts the name's repository id beside the login's account and the placement.
   - With the repository guessed, `fresh_gate`'s reads (access rows plus the owner's `Authz`) can start beside
     `resolve`'s, on the guessed repository and record.
   - They are kept only if `resolve` answers the same repository and record, as `chained` does. Otherwise they are
     dropped and the gate reads again.
   - The decision is still made on the fresh reads' answers, and the decision's stamp is their fold read. So the
     revocation bound and the rule that a refusal is read fresh are unchanged.
   - This removes about 860 ms from a cold hit: the gate's wall overlaps the front.

**Plants that would guard (a):**
- a guessed gate read kept when `resolve` named another repository or record;
- a guess served as an answer, with no fresh read;
- the decision stamped at the guess's time, not the fold read's;
- a refused gate guessed and not re-read;
- a colo link entry served past `READ_CACHE` as an answer.

These are `chained`'s existing tests plus a new `beside_gate` test in the same shape as
`the_shared_gate_serves_by_the_fold_reads_stamp_on_every_path`.

**Cost (a):**
- No new store: the Cache API is already a product row, with failure modes for every class.
- One more `match` and `put` per cold request.
- Member reads unchanged: a guessed read is kept or re-read once.
- Build: about 150 lines in the edge (`resolve`, `read_gate`, three colo guess keys) and two tests. Edge day only.

## (b) `chain_open` on a new head version

mt15's `chain_open` by how many packs the hit checked:

| Packs checked (`chain_chk`) | Hits | `chain_open` p50 / p99 / max |
|---|---|---|
| 0 (every pack from the set) | 285 | 92 / 194 / 273 ms |
| 1 | 12 | 253 / 449 / 449 |
| 2–8 | 21 | 280–378 / up to 521 |
| 11–20 | 29 | 469–695 / up to **1,209** (the 1.21 s hit: 11 checked) |

A hit checks many packs only when its isolate has no set for the head version it read:
- the first hit after a fold, whose head names a new base;
- an isolate's first hit on a version, when the data center has no set for it yet either.

Every `tip:move` (about 1 a second on j9) puts a new head with a new version and one new part, so a set is rebuilt per
data center per version. Most of each rebuild re-checks packs the previous version's set already checked.

**Can a new version's set reuse the previous version's results?** Yes, but not by pack key alone.
1. **What changes between two consecutive heads.**
   - The builder puts each new file, then the head (`ready_chain`, `crates/git/src/host.rs`).
   - A part step adds `part-<old tip>-<new tip>` and keeps every other file under the same key and the same bytes.
   - A fold writes a new `base-<tip>` and deletes the old files after the head that replaces them.
   - A step rebuilt after a container restart can rewrite an existing key with new bytes, with the same name and a
     new etag. That is the case `colo-gate` keyed sets by version for.
2. **What would make carrying forward safe:** carry an entry from the version-v set into v+1 only when it is still
   the same object.
   - **Count:** the manifest's `n` must equal the set's `n`, as today (`check_with_sets`).
   - **Same object:** the entry's etag must equal the object's current etag. Today only a check reads the etag (a
     ranged get). Two ways to learn it without a check:
     - **(b1)** one R2 `list` of `compose/<net>/` beside the head get. A `list` returns every object's key, size and
       etag, and R2 is strongly consistent for list after put [doc,
       https://developers.cloudflare.com/r2/api/workers/workers-api-reference/: "R2 writes are strongly consistent.
       Once the Promise resolves, all subsequent read operations will see this key value pair globally"]. An entry
       whose key, etag and size match is carried; any other pack the head names is checked.
     - **(b2)** the builder writes each file's etag, size and count into the head itself, so the head names exactly
       the objects it was put after. The head is put after its files, so the etags it names are the ones R2 holds,
       unless a later put replaced them. The edge's stream is conditional on that etag (`etag_matches`), so a
       replaced pack ends the answer short, as today.
   - **A pack replaced under the same key:** it has a new etag, so (b1)'s list shows the new etag and the entry is
     not carried. Under (b2) the new head names the new etag. A pack replaced after the edge's list or head read ends
     the answer short on the conditional get, as today. No case streams a pack the head does not name at the etag
     checked.
   - **R2 version semantics:** the set is still keyed by the head's version, so a set never outlives its head. The
     carried entries are copied into the new version's set only after their etag matched. Version answers "which
     head", and etag answers "which bytes of each pack".
3. **What each costs and predicts** (a hit that checked more than one pack checks only the new one):

| Lever | `chain_open` on a new version | j9 measured p99 | j9 outages removed | Extra per hit |
|---|---|---|---|---|
| (b1) carry by `list` | ≤ 550 ms (one check plus a list) | 2,189 | 1,060 | one Class A `list` per new version per isolate |
| (b2) head names etags | ≤ 200 ms (no check) | 2,189 | 746 | none at the edge; the builder writes etags into the head |

- (b1)'s `list` is Class A, $4.50 per million. The set is shared in the data center, so the list runs about once per
  new version per colo, plus isolates that race before the first put lands. At one version a second that is about
  3,600 an hour per colo per hot network, about $0.02 an hour. But it adds a list's wall (not measured; one R2 `get`
  on the edge is 50–150 ms) to every first hit on a version.
- (b2) needs a site-script change (the builder writes the head) and an edge change (read the etags from the head and
  skip the check). It ships only through a site day, since the builder runs in the site's shard object.
- The head grows by one etag, size and count per file, at most `foldEvery` (20) files, so under 2 KB.
- The builder knows each new file's etag from its `put`'s answer. For a file it does not re-put (`to_fetch` skips
  files the bucket's old head names), it copies the old head's fields. The first head after this ships has none, so
  that one step `head`s each file once.
- (b2) is the better lever: no per-hit cost, and it removes the check entirely for every pack the head names. The
  edge still streams each body on its etag, so a fold or a rewrite after the head read cuts the answer as today.

**Plants that would guard (b):**
- an entry carried when its etag differs from the head's (or the list's);
- an entry carried when the count differs;
- a pack the new head does not name served from a carried entry (a fold);
- a set keyed by pack key alone;
- the edge's body read without `etag_matches`.

These are `check_with_sets`' existing mutants (plant79 11 and 12) plus a new test,
`a_head_named_etag_answers_only_while_it_is_the_objects_etag`.

**One trap in (b2):** `ChainHead` is compared whole in `ready_chain`, in both `synced == Some(&head)` and
`had == head`. The container's `/compose-head` answers without the new fields. So the per-file fields must live
beside the chain, in a `files` map that the comparisons ignore, or every step re-puts the head. The Go side ignores
unknown JSON fields on `PUT /compose-head`, so no container change is needed [inference: `json.Decoder` without
`DisallowUnknownFields`, `containers/git/server/compose.go`].

**Cost (b2):** about 60 lines in the builder (`ready_chain`, `ChainHead` gains optional per-file fields) and about 40
in `check_with_sets`. It is a wire change to `compose/<net>/head`: the new fields are optional, so an edge that does not
know them checks as today, and an older head without them is checked as today. Plus a site day and an edge day.

## Together

| Levers | j9 measured p99 | over 1.1 s | j9 outages removed |
|---|---|---|---|
| (a) `beside` + (b1) carry | 1,075 | 1 | 1,060 |
| (a) `beside` + (b2) head etags | **1,027** | 1 | 746 |
| (a) `beside` + (b2), cold reads at ½ front instead of ⅓ | 1,027 | 1 | 746 |
| a real deploy: `links` + warm entry + (b2) | 773 | 0 | 746 |

The measured p99 would sit on its third and fourth slowest hits:
- the first-second hits' gate (856–1,031 ms), now beside a ⅓ front, plus client and rest;
- `chain_open` on the 93 ms hit.

The margin under 1.1 s is about 75 ms on one row [inference: mt12 to mt14's first seconds had fronts of 871–891 ms
and gates of 146–1,163 ms, the same shape].

**What would close it with margin, if (a) + (b2) lands near 1.1 s:** the first-second gate itself, at 856–1,031 ms
for one member read on a cold isolate.
- That read is `gate_rows` (the repository's access rows) on a learner the isolate has no answer-time average for
  yet, so it picks one at random.
- A cold isolate could send its first read of a group to the member the colo's last isolate found fastest, kept as a
  guess in the Cache API.
- That is a third lever; I have not modeled it, because the samples carry no per-member wall for these four.

## Recommendation

1. Build (b2) first: the head names each file's etag, size and count.
   - It helps every hit after the first second (outages removed 1,279 → 746 ms), costs nothing per hit, and keeps
     every bound.
   - It needs a site day and an edge day.
2. Build (a) `beside` with it: start the gate's reads on the guessed repository beside `resolve`.
   - It is the only first-second lever that works on the harness's new repository and new edge.
3. Don't build the edge-day warm of the shared gate entry. It can't hold a client's credential, it would be 50 minutes
   stale by t0, and a real deploy keeps the data center's entries across the swap.
4. The row after both merge predicts measured j9 at about 1,030 ms. If it lands over 1.1 s, the next lever is the cold
   isolate's member pick for its first gate read.
