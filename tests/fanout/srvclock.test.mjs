// srvclock.mjs against a simulated server whose clock runs a known offset ahead of a fake orb clock:
//   node --test tests/fanout/srvclock.test.mjs
import { test } from "node:test";
import assert from "node:assert/strict";
import { bound, measure } from "./srvclock.mjs";

/** A fake orb clock and a server `offset` ms ahead of it; each request takes `up` ms to arrive and `down` to return. */
function world(offset, up, down, start = 1_795_000_000_123) {
  let t = start;
  return {
    clock: () => t,
    wait: async (ms) => { t += Math.max(0, ms); },
    fetchFn: async () => {
      t += up;
      const server = t + offset;
      t += down;
      return { headers: new Map([["date", new Date(Math.floor(server / 1000) * 1000).toUTCString()]]) };
    },
  };
}

test("a server ahead of the orb gives a positive offset within its error", async () => {
  for (const [offset, up, down] of [[7_400, 20, 30], [7_463, 5, 60], [-2_250, 25, 25], [130, 40, 10]]) {
    const w = world(offset, up, down);
    const m = await measure("x", { ...w, rounds: 12, goalMs: 30 });
    assert.ok(Math.abs(m.offsetMs - offset) <= m.errMs + 1, `${offset}: got ${JSON.stringify(m)}`);
    // The answer must be tighter than the Date header's one-second resolution, or the probe adds nothing.
    assert.ok(m.errMs <= up + down, `${offset}: error ${m.errMs} wider than the round trip`);
  }
});

test("one sample bounds the offset by the header's second and the round trip", () => {
  // Sent at orb 1000, back at 1100, header 9 s: the server read 9000..9999 at orb 1000..1100.
  assert.deepEqual(bound([{ sent: 1_000, received: 1_100, date: 9_000 }]), { lo: 7_900, hi: 9_000 });
  assert.equal(bound([{ sent: 0, received: 10, date: 5_000 }, { sent: 0, received: 10, date: 9_000 }]), null);
});
