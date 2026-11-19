#!/usr/bin/env python3
"""tier-read-batch's planted bugs: each must fail one of the `rcache` tests (graftyard-edge). Restores each file byte for
byte. Run from the worktree root: `python3 tests/tierbatch/plant.py`. The first four are main's behaviour or a wrong
batch; the last hands a waiter the answer of the read in flight."""
import subprocess

RC = "workers/edge/src/rcache.rs"
AG = "workers/edge/src/agents.rs"
TESTS = ["cargo", "test", "-q", "-p", "graftyard-edge", "--lib", "--", "rcache::tests::writes_",
         "rcache::tests::a_cached_gate_push"]
BATCH = "batched_from(cache, k, Timestamp(0), need.get(), until, read, now, delay).await"
NEED = "    let need = std::cell::Cell::new(cache.with(|c| c.borrow().next_id()));"
NEXT = "            need.set(id + 1);"
TAKE = """        if let Some((id, r)) = &f.done
            && *id >= need
        {"""
KEY = "    (chain, q.to_string())"
CTX = "    crate::api::push_rows_q()\n}"
M = [("main: no batch, each write runs its own passes", RC, BATCH,
      "Some((Batched { answer: read.await, shared: false, waited_ms: 0.0 }, need.get()))"),
     ("after a shed, a waiter takes the pass it already saw", RC, NEXT, "            need.set(id);"),
     ("the batch is keyed by chain alone", RC, KEY, "    (chain, String::new())"),
     ("a cached-gate push reads its agent rows alone (main)", AG, CTX, "    serde_json::json!({ \"agent\": true })\n}"),
     ("a waiter takes the answer of the read in flight when it arrived", RC, TAKE,
      """        if let Some((id, r)) = &f.done
            && *id + 1 >= need
        {"""),
     ("a call takes any answer, even one read before it arrived", RC, NEED,
      "    let need = std::cell::Cell::new(0u64);")]
for name, f, old, new in M:
    s = open(f).read()
    assert s.count(old) == 1, name
    open(f, "w").write(s.replace(old, new))
    try:
        code = subprocess.run(TESTS, capture_output=True, text=True, timeout=1200).returncode
    finally:
        open(f, "w").write(s)
    print("CAUGHT" if code else "MISSED", name, code, flush=True)
