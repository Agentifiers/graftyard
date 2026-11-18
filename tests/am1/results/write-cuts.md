# write-cuts: the two cuts from tx-mix

These are the cuts the orchestrator approved after tx-mix (main 3301bf3e). Cut 1 is built. Cut 2's approved location
breaks F88's rule, so its code is held and the alternative below goes to the operator first.

## Cut 1: built

- **`dup`.** The request insert already ends `ON CONFLICT (rid) DO NOTHING`, so an insert that writes nothing is a
  request id already on disk.
  - On test installs, `commit_tx` reads that insert's `rows_written` (`workers/site/src/txn.rs`). It writes a `dup`
    row in the same transaction only when the insert wrote nothing.
  - So a request costs one statement, not two, and BF-12's evidence stays in storage across a reset, as the operator
    asked.
  - `GroupFold::apply`'s in-memory refusal (`RequestStore::put`) is unchanged.
- **`arrival=mem`.** It is set for every B0-33 row from now on.
  - `rows.sh` sets `/stats?arrival=mem` on every voter before B0-33's load (`tests/fanout/arrival.mjs`) and skips the
    row unless all three confirm it.
  - `memguard.mjs` (from `b033.sh`) sends the mode with every sample, so a voter that reset is back on it within 2 s,
    and each sample records `arrival`.
  - The summary `/stats` now reports `arrival` (test-faults).
  - `tests/fanout/README.md` says B0-33 m50 to m52 ran with `sql` and are not comparable on confirm time.
- **Tests.**
  - `txn::tests::only_a_request_insert_that_wrote_nothing_is_a_double_apply`.
  - `tests/writecuts/bed/run.sh`: the member's own `txn.rs` in local workerd. A first apply leaves no `dup` row. A
    second apply of the same request id at index 9 leaves `r1@9`. A new request id adds nothing. `r1@9` is still
    there after a workerd restart. PASS. With `PLANT=1` (no feature, so no `dup` write) it FAILs.
- **Plants.** `tests/writecuts/plant.py`: 5 of 5 caught.
  - the dup write dropped (the bed);
  - every insert counted as a double apply;
  - an insert that wrote only its index entry counted as a double apply;
  - the index left out of the `dup` row;
  - `arrival.mjs` passing a voter that answered `sql`.
- **Checks.** fmt; workspace clippy; wasm32 clippy for site and edge with and without `test-faults`, and for ops;
  `cargo test --workspace`; `go test`; check_design, check_names, check_links. All pass.
- **Ships.** The `dup` change ships at the next site day. `arrival=mem` needs no site day: m52's build already has the
  route.

## Cut 2: the approved location breaks F88

The floor was approved in `lease::extend` (`crates/git`), applied in both the scratch and the tier apply. That changes
what an entry **already committed** folds to.
- Every m52 and m50 entry whose `extend.by` is under 5 s moves leases on today's release and would move none on the new
  one.
- F88's rule is that a committed entry folds the same on every release (`crates/fold/src/tier.rs`, `Tiered::write`;
  `decisions/DECISIONS.md`, "F88").
- Breaking it would show twice:
  - **During a site day's roll.** A member already on the new release and one still on the old one apply the same
    entry differently, so their tier digests differ.
  - **Afterwards.** The fold audit's refold of any chain with a sub-floor extend in its tail alarms, F88's own shape.
- **The corpus would not catch it.** `fold_corpus.rs` builds every fixture with `extend: None`.

Gating the floor by the entry's `format` does not work cleanly either:
- `extend` rides any entry, including `Noop`, config and id-block entries at `RAFT_FORMAT`.
- `on_extend` does not get the format.

### The alternative: the floor where the extension is stamped

**Rule.** The leader's `take_extend` (`crates/raft/src/member/leader.rs`) stamps no `extend` when `by < EXTEND_MIN`
(5 s [guess]). The marker then clears as it already does: an entry stamped after the marker commits (`set_commit`).
- No committed entry folds differently, because entries simply carry no sub-floor `extend`.
- Path leases and Actions job leases follow one rule with no second change: both read only `extend`.
  `crates/actions`' `extended` stays as it is.
- Cost: it is a `crates/raft` change, so the design comes first, then the long sweep and Porcupine. Raft safety does
  not read `extend`, but the sim's histories carry the marker.
- **One open point.** Today, `since` is at least the last entry's `at`. A long stall broken by one sub-floor gap is one
  `extend` covering the whole stall, so the floor applies to `now - since` and not to each gap. A real 30 s outage
  still moves leases by 30 s.

**Measured size of the change** (the gap between the leader's appends before each 1,000+-statement transaction, from
`raw/tsave-m5{0,2}b33.jsonl.xz`'s `Stepped` rows):
- m52 site a: 9 gaps from 0.5 to 3.8 s, 15.9 s in all over 214 s of leading.
- m52 site b: 16 gaps from 0.06 to 3.8 s, 24.0 s over 68 s.
- m50 site b: 10 gaps from 0.2 to 4.3 s, 17.4 s over 184 s.
- No append gap on any leader reached 5 s (max 4.6 s). So at 5 s, **every** `ExtendLeases` in these rows would go.

**The slack, as the operator corrected it.** A lease that is not renewed lapses earlier than today by the sum of
sub-floor stalls since its last renewal. At these rates over a 600 s TTL:
- about 45 s at m52 site a's rate;
- about 57 s at m50 site b's;
- about **211 s** at m52 site b's.

Such a lease then lapses at its wall-clock `expires`, as on an idle group. A holder that renews is unaffected.

**Tests.**
- Repeated sub-floor stalls: three 3 s stalls before an unrenewed lease expires, and it lapses at its wall-clock
  `expires`. Main moves it by 9 s, so the test fails on main.
- A 6 s stall still moves it by 6 s.
- A long stall with a sub-floor tail moves by the whole stall.
- Both lease kinds: one path lease and one Actions job lease through the same entries.

**Plants.**
- `>` for `>=`.
- The floor applied to the last gap instead of `now - since`.
- The marker kept after a dropped stamp, so the next extend covers the dropped time.

**Design edits.** DESIGN, Shape (`ExtendLeases`). AGENT-LAYER, "Leases" (the cumulative-slack sentence, [inference]).
A DECISIONS entry with the operator's flip. A note in SHARED's Actions leases line that the floor applies.
