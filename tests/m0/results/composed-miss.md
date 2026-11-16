# Composed clone-cache misses on the shard container (2026-11-15)

What this measures: the container's composed miss (`containers/git/server/compose.go`) against stock `upload-pack` on a linux-sized network. It is the local check behind `decisions/DECISIONS.md`, "Clone-cache misses are composed from the cached tip" and "The clone cache fills from the shard". The hosted check is LM-1's next B0-34 run.

## Setup

- Fixture: a bare single-branch clone of torvalds/linux at `23c62304`, one 6.46 GiB pack, 12.0M objects.
- The shard's repository borrows the fixture's objects through alternates.
- The chain is the 61 first-parent commits ending at `23c62304`, oldest first (`git log --first-parent --format=%H -n 61 HEAD | tac`). The base is the oldest; each later commit is one `/compose` tip move.
- The orb has 4 vCPU and git 2.56.0. CPU is the rusage of the git processes the server ran, and wall time is the test client's.
- Test: `TestLinuxComposedMissCost` (`containers/git/server/compose_linux_test.go`). The raw log is `tests/m0/results/raw-carried/composed-miss-linux.log`.

```
cd containers/git/server
GRAFT_LINUX=/path/to/linux.git GRAFT_LINUX_CHAIN=chain.txt go test -run TestLinuxComposedMissCost -count=1 -v -timeout 30m .
```

## Chain builds

| Step | CPU | Notes |
|---|---|---|
| Base at `b556f1c4` | 10.2 to 11.1 s | `pack-objects --window=0 --delta-base-offset` over the tip's trees and blobs |
| Part, per tip move (57 moves) | p50 0.02 s, max 3.2 to 3.5 s | The max is move 25 (`e0d8e8de`, 7,748 new objects); move 46 (448 objects) took 0.45 s |
| Fold, every 20 moves | 1.49 to 1.58 s | Moves 20, 40 and 60 |

## Misses

| Miss | First byte | First 64 KiB of pack | Wall | Server CPU | Bytes |
|---|---|---|---|---|---|
| Composed, tip | 0.011 to 0.013 s | 0.011 to 0.014 s | 0.85 to 0.97 s | 0.01 to 0.02 s (git children); 1.5 to 1.8 s with the Go server's SHA-1 and side-band work and the client's read | 302,040,346 |
| Composed, pull head (one commit past the tip) | 0.013 to 0.017 s | 0.013 to 0.018 s | 0.77 s | as above, 1.4 s in all | 302,066,067 |
| Stock, tip | 0.005 s (shallow-info) | 4.96 s | 15.58 s | 29.9 s | 301,048,241 |
| Stock, pull head | 0.002 s | 5.10 s | 15.35 s | 30.2 s | 301,047,600 |

The composed pack is 0.3% larger than stock's. On the client, `index-pack --strict`, `fsck --strict` and a checkout of the composed pack took 25.5 to 26.5 CPU-s; that cost is the client's either way.

## Compared with M0-WP9

M0-WP9's prototype (`tests/m0/wp9/git/main.go`) measured 0.72 to 0.74 s a composed miss and a time to first byte of 0.35 s (`tests/m0/results/m0-wp9.md`, B0-34).

- Wall time here is 0.77 to 0.97 s for 300 MB. The server copies the chain's files, so the client's read bounds it, and its own git CPU is 0.01 to 0.02 s.
- The first byte comes in 0.011 to 0.017 s against 0.35 s. This server writes the pack header at once and streams each pack's body from its file. Why the prototype's first byte came later was not measured.
- Stock's CPU, 28 to 30 s, is within B0-34's 25 to 46 CPU-s, and the first 64 KiB of stock's pack comes after about 5 s of delta search.

## Correctness

These checks are in `TestComposedMiss` (`containers/git/server/compose_test.go`). Each composed answer goes through a client's `index-pack --strict` and `fsck --strict` in an empty repository, with a tree equal to the wanted commit's and a checkout. They cover:

- a base and parts through a rename, a delete, a move, and a reverted file;
- the tip and a pull request's head;
- a fold;
- a second shard that installs the chain through `/compose-file` and `/compose-head`.

These fall back to stock, each with its `x-graft-compose` reason:

- a want no ref names, a filter, a have, no `ofs-delta`, `deepen 2`;
- `include-tag` with an annotated tag on the want;
- a chain behind HEAD;
- a shard with no chain.

`TestCancelledFillStillSpoolsTheWholeAnswer` covers a stock fill and a composed fill whose client cancels after one byte. Each still leaves the whole answer spooled with its length.

## A want away from the chain tip (2026-11-16)

After LM-1 round 26, a chain behind HEAD and a want behind HEAD compose: the chain plus the want's diff from the chain tip, packed with no delta search. Same fixture and test, the raw log in `tests/m0/results/raw-carried/composed-miss-linux-2.log`:

| Miss | First byte | Wall | Bytes |
|---|---|---|---|
| Composed, tip | 0.010 to 0.011 s | 0.95 to 1.10 s | 302,040,346 |
| Composed, want 10 moves from the chain tip | 0.255 s | 1.11 s | 304,118,315 |
| Composed, want 30 moves from the chain tip | 0.836 s | 1.64 s | 311,257,919 |
| Stock, tip | 0.005 s (first 64 KiB of pack 5.34 s) | 16.77 s | 301,048,251 |

The 30-move answer passed the client's `index-pack --strict`, `fsck --strict` and a checkout with a tree equal to the want's. The first byte grows with the diff, because the want's own pack is built before the header that counts it.

## The per-miss line (2026-11-16)

Each miss's shard object now logs `shard miss <net> <CloneKey digest>: {...}` (`graftyard_git::compose::MissLog`): its arrival in epoch ms, then in ms after it `ready_ms` (the chain wait), `only` (the `compose=only` answer's ms, status and `x-graft-compose`), `caught_up_ms` (stock only), `headers_ms` (the serving answer's headers, which the container sends with its first byte) and `first_ms` (its first chunk on to the edge), and `answer`. A probe finds its line by the key it already computes (`cloneKeyHex` in `tests/am1/rest/a1.mjs`). In `tests/composed/bed/run.sh` with `FWD=1`, a 300 MiB answer passed through the forward whole, the callback ran once, and the wasm memory stayed at 1 MiB.
