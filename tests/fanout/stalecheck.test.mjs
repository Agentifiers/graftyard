// stalecheck.mjs on hand-built push logs: node --test tests/fanout/stalecheck.test.mjs
import { test } from "node:test";
import assert from "node:assert/strict";
import { judge } from "./stalecheck.mjs";

const R = "refs/heads/claude/a1-r-7";
const p = (client, start, end, old, nu, status, result, ref = R) => ({ client, kind: "agent", ref, old, new: nu, start, end, status, result });
const committed = (client, index) => ({ client, status: 200, end: { index, refs: ["Ok"] } });

test("a Stale after an acked push at the same tip fails the run", () => {
  const v = judge([p("c1", 0, 900, "a", "b", 200, "ok"), p("c2", 1000, 1900, "b", "c", 200, "stale")]);
  assert.equal(v.verdict, "FAIL");
  assert.deepEqual([v.fail, v.pass], [1, 0]);
});

test("a Stale after a lost reply the edge shows committed passes", () => {
  const pushes = [p("c1", 0, 900, "a", "b", 0, "http 0 TypeError: fetch failed"), p("c2", 1000, 1900, "a", "c", 200, "stale")];
  assert.equal(judge(pushes, [committed("c1", 41)]).verdict, "PASS");
  // Without the edge's line the same log cannot say which: never a pass.
  assert.equal(judge(pushes).verdict, "UNKNOWN");
  // The edge answered it, but the leader refused it: the ref did not move, so the Stale is wrong.
  const refused = { client: "c1", status: 200, end: { index: 41, refs: [{ Rejected: "Stale" }] } };
  assert.equal(judge(pushes, [refused]).verdict, "FAIL");
  // A commit that ran out of time at the edge has no leader answer: it may have committed later.
  assert.equal(judge(pushes, [{ client: "c1", status: 500, end: null }]).verdict, "UNKNOWN");
});

test("a Stale after a refused push at the same tip fails, and pushes to other refs or overlapping ones do not decide it", () => {
  const full = [p("c1", 0, 900, "a", "b", 200, "full"), p("c2", 1000, 1900, "a", "c", 200, "stale")];
  assert.equal(judge(full).verdict, "FAIL");
  // An acked push to another ref at the same tip is not this ref's previous push.
  const other = [p("c0", 0, 900, "z", "a", 200, "ok", "refs/heads/x"), p("c2", 1000, 1900, "a", "c", 200, "stale")];
  assert.equal(judge(other).verdict, "UNKNOWN");
  // Two humans on one ref at once: a race, not a lost write.
  const race = [p("h1", 0, 1500, "a", "b", 200, "ok"), p("h2", 1000, 1900, "a", "c", 200, "stale")];
  assert.equal(judge(race).rows[0].why.startsWith("race"), true);
  assert.equal(judge(race).verdict, "PASS");
});
