<h1>
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset=".github/assets/graftyard-logo-dark.svg">
    <img alt="Graftyard" src=".github/assets/graftyard-logo-light.svg" height="72">
  </picture>
</h1>

Graftyard is a git host that unmodified GitHub clients (`git`, `gh`, Actions runners, Terraform, Octokit, and AI coding agents) treat as GitHub Enterprise Server (GHES) 3.22. It runs only on Cloudflare products.

<p>
  Built by&nbsp;
  <a href="https://agentifiers.com"><picture>
    <source media="(prefers-color-scheme: dark)" srcset=".github/assets/agentifiers-logo-dark.svg">
    <img alt="Agentifiers" src=".github/assets/agentifiers-logo-light.svg" height="36" align="absmiddle">
  </picture></a>
</p>

- **Uptime first.** Every repository is a Raft group of three SQLite-backed Durable Objects on three sites in three regions.
- **No lost pushes.** A push is acknowledged only after two sites in two regions hold it.
- **One artifact, three modes.** Managed, bring-your-own-cloud (BYOC), and self-hosted.
- **Built for agents.** Agents use Graftyard as ordinary GHES clients. An agent layer adds leases, an activity feed, a task API, and host-enforced agent rules, without changing what other clients see.

## Status

Graftyard is under active development and is not ready for production use.

Milestones 0 to 3 are closed: platform measurements, Raft groups and git, folds, issues, pull requests and merge, and identity and authorization. Each milestone passed its gate on real Cloudflare installs with real `gh`, `git`, `git-lfs`, and Terraform clients. Work on the agent layer's load gate and on Milestones 4 and 5 (webhooks, Actions, the web app, and release control) is under way. `build/BUILD-PLAN.md` holds the status table for each milestone. Any number that no run has measured is tagged [guess], [inference], or [unverified].

## What has been tested

`tests/RESULTS.md` lists every run with its faults, checks, and source, and defines each number below.

- Porcupine checked 12,304,207 operations for linearizability over 104.6 hours of chaos on Cloudflare, under 16,258 member resets and 10,680 leadership transfer attempts.
- 0 of 3,588,424 acknowledged writes were lost in the runs whose member logs were audited.
- 12,000 leader kills between quorum and reply each returned the original outcome.
- `clients.sh` passed 29 of 29 rows in 12 runs on git 2.39.5, 2.51.0, and 2.56.0, 6 of them with a site failing.
- The Raft simulator's long sweep ran 40,191,804 steps, and every one of its 1,000,856 acknowledged writes carried valid ack evidence.
- The test suites caught 84 of 85 planted bugs.

## Repository layout

```
design/        the design of record, the shared specification, the agent layer, and the glossary
build/         BUILD-PLAN.md: work packages, their order, and the gate that finishes each
decisions/     why each choice was made, what would reverse it, and open questions
research/      the evidence behind the design's citations, and the cost reference
alternatives/  the AWS design that lost, priced beside this one
crates/        Rust libraries: raft, git, identity, fold, api, wire, actions, release, lookup, types
workers/       Cloudflare Workers: front, edge, site, ops, runners
cli/           the graftyard install and operations CLI
containers/    the git cache shard and Actions runner images
web/           the React web app
site/          the graftyard.com website
tests/         gate harnesses, results, and raw run data, by milestone
scripts/       design checks and cost models
history/       the verbatim record of the original design run
```

## Reading the design

1. `FRAME.md`: the goals in priority order, the constraints, and every operator decision.
2. `design/DESIGN.md`: the design of record: sites, Raft groups, the commit and read paths, failure modes, deployment modes, and cost.
3. `design/SHARED.md`: the platform-independent specification: wire compatibility, events, tokens, issues and pull requests, folds, and Actions.
4. `design/AGENT-LAYER.md`: Agent Apps, the MCP server, leases, the activity feed, and the task API.
5. `build/BUILD-PLAN.md`: the work packages and their gates.
6. `decisions/DECISIONS.md` and `decisions/OPEN-QUESTIONS.md`: settled choices and the questions that remain.

`design/GLOSSARY.md` defines terms such as site, group, fold, `decide`, and site day. `AGENTS.md` holds the rules for the coding agents that build Graftyard.

## Building and checking

Rust (stable, with the `wasm32-unknown-unknown` target), Node.js 24, Go, and Python 3 are needed. CI (`.github/workflows/ci.yml`) runs these:

```
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
(cd web && npm ci && npm run check)
python3 scripts/check_design.py
python3 scripts/check_names.py
python3 scripts/check_links.py
python3 scripts/check_plants.py
```

| Script | What it does |
| --- | --- |
| `scripts/check_design.py` | Checks both design documents' structure, the Stack table against `research/cloudflare.md`, failure-mode and deployment rows, that the agent block is identical in `design/DESIGN.md` and `design/AGENT-LAYER.md`, and that the Cost sections match the cost scripts |
| `scripts/check_names.py` | Checks that the project's old name appears nowhere outside `history/` |
| `scripts/check_links.py` | Checks that every repository path cited outside `history/` exists |
| `scripts/check_plants.py` | Checks that every planted bug's anchor matches its code exactly once, against `scripts/plants-baseline.txt`, which only shrinks. A match is necessary, not sufficient: only running a plant shows its mutants compile and are caught |
| `scripts/cost.py` | Prints the base's monthly Cloudflare list cost at 50, 300, and 2,000 developers |
| `scripts/cost_agents.py` | Prints the agent layer's added monthly cost |
| `scripts/cost_model.py` | The AWS cost model, which also holds the workload every cost script uses |
| `scripts/costs_report.py` | Regenerates `research/costs.md` from the cost scripts |
| `scripts/census.py` | Counts GitHub wire surfaces into `research/surfaces.json` |

### Planted bugs

Each `tests/**/plant.py` plants one mutant at a time, runs the tests that guard it, and restores the file byte for byte. Plants do not run in CI; run the ones whose code a change touches. Every plant follows the same rules:

- Each test run goes through `timeout -k 10 600` (longer only for a bed that builds a Worker), so a mutant that leaves a test looping is killed with its whole process group. A run the timeout ended counts as caught, since the suite did not pass.
- Builds go to `CARGO_TARGET_DIR`, which defaults to a directory under `target/`, inside the repository and git-ignored.
- A mutant whose code a later change replaced is planted on the replacement rule, or dropped with a one-line reason in the plant file.

## How the design was produced

A multi-agent design run wrote the design from 2026-10-23 to 2026-10-27. Research agents wrote `research/`. Arena rounds pitted competing designs against each other and folded the strongest ideas of the losers into the winner, and adversarial reviews and implementability walks closed the gaps they found (`decisions/DECISIONS.md`). `history/README.md` explains the record.

## License

Graftyard is released under the [MIT License](LICENSE).
