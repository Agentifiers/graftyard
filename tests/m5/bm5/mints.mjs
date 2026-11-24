// BM-5 clause 3 (`mint.sh load`): n installation-token mints for m3-ci on m3org through the edge, c at a time. Prints
// one JSON line counting the answers by status ("throw" for a call that never answered). No token reaches the output.
const { ghs } = await import(`${process.cwd()}/tests/am1/hosted/lib.mjs`);
const [n, c] = process.argv.slice(2).map(Number);
const counts = {};
let next = 0;
await Promise.all(Array.from({ length: c }, async () => {
  while (next < n) {
    next++;
    const r = await ghs().catch(() => ({ status: "throw" }));
    counts[r.status] = (counts[r.status] ?? 0) + 1;
  }
}));
console.log(JSON.stringify(counts));
