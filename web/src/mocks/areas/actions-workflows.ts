import type { Schemas } from "@/api/client";
import type { DispatchInput } from "@/areas/actions/ui-api";
import { branches, tags } from "../world";

// The workflow files in each repository, parsed: triggers, dispatch inputs, jobs with
// their `needs`, and what each step prints. actions-db.ts runs these, either at once
// (seeded history) or line by line on a timer (live runs).

export type Ctx = {
  full: string;
  branch: string;
  sha: string;
  attempt: number;
  event: string;
  inputs: Record<string, string | boolean>;
};

export type StepSpec = { name: string; lines: (c: Ctx) => string[] };

export type Failure = {
  // 0-based index into the job's steps.
  step: number;
  lines: string[];
  annotations: Omit<Schemas["check-annotation"], "blob_href">[];
};

export type JobSpec = {
  key: string;
  name: string;
  needs?: string[];
  // The job's `if:`; a job whose condition is false is skipped.
  when?: (c: Ctx) => boolean;
  environment?: string;
  runsOn: string[];
  steps: StepSpec[];
  fails?: (c: Ctx) => Failure | null;
  artifact?: { name: string; size: number };
};

export type WorkflowSpec = {
  file: string;
  name: string;
  events: string[];
  dispatch: { name: string; input: DispatchInput }[] | null;
  // What the workflow's first step rejects about its inputs: the field and the 422 code.
  checkInputs?: (
    full: string,
    inputs: Record<string, string | boolean>,
  ) => { field: string; code: string } | null;
  jobs: JobSpec[];
  state?: Schemas["workflow"]["state"];
  created: string;
  source: string;
};

// ANSI colours as cargo, npm, and the runner print them.
const C = {
  green: (s: string) => `\x1b[32m${s}\x1b[0m`,
  boldGreen: (s: string) => `\x1b[1m\x1b[32m${s}\x1b[0m`,
  red: (s: string) => `\x1b[31m${s}\x1b[0m`,
  boldRed: (s: string) => `\x1b[1m\x1b[31m${s}\x1b[0m`,
  yellow: (s: string) => `\x1b[33m${s}\x1b[0m`,
  cyan: (s: string) => `\x1b[36m${s}\x1b[0m`,
  blue: (s: string) => `\x1b[34m${s}\x1b[0m`,
  dim: (s: string) => `\x1b[90m${s}\x1b[0m`,
  bold: (s: string) => `\x1b[1m${s}\x1b[0m`,
};

const setUp = (runsOn: string[]): StepSpec => ({
  name: "Set up job",
  lines: () => [
    "Current runner version: '2.329.0'",
    `Runner name: '${runsOn.includes("gpu") ? "gpu-runner-01" : "edge-runner-02"}'`,
    "Runner group name: 'Default'",
    `Machine name: 'gy-${runsOn.join("-")}'`,
    "##[group]GITHUB_TOKEN Permissions",
    "Contents: read",
    "Metadata: read",
    "PullRequests: read",
    "##[endgroup]",
    "Secret source: Actions",
    "Prepare workflow directory",
    "Prepare all required actions",
    "Getting action download info",
    "Download action repository 'actions/checkout@v4' (SHA:11bd71901bbe5b1630ceea73d27597364c9af683)",
    "Complete job name: " + runsOn.join(", "),
  ],
});

const checkout: StepSpec = {
  name: "Run actions/checkout@v4",
  lines: (c) => [
    "##[group]Run actions/checkout@v4",
    "with:",
    `  repository: ${c.full}`,
    "  token: ***",
    "  fetch-depth: 1",
    "##[endgroup]",
    "Syncing repository: " + c.full,
    "##[group]Getting Git version info",
    "git version 2.47.1",
    "##[endgroup]",
    `Initializing the repository`,
    `[command]/usr/bin/git init /home/runner/work/${c.full.split("/")[1]}`,
    "##[group]Fetching the repository",
    `[command]/usr/bin/git -c protocol.version=2 fetch --no-tags --prune --depth=1 origin +${c.sha}:refs/remotes/origin/${c.branch}`,
    `From https://ghes.graftyard.test/${c.full}`,
    ` * [new ref]         ${c.sha.slice(0, 7)} -> origin/${c.branch}`,
    "##[endgroup]",
    `HEAD is now at ${c.sha.slice(0, 7)}`,
  ],
};

const rustToolchain: StepSpec = {
  name: "Run dtolnay/rust-toolchain@stable",
  lines: () => [
    "##[group]Run dtolnay/rust-toolchain@stable",
    "with:",
    "  toolchain: stable",
    "  components: rustfmt, clippy",
    "##[endgroup]",
    C.bold("info:") + " syncing channel updates for 'stable-x86_64-unknown-linux-gnu'",
    C.bold("info:") + " latest update on 2026-10-30, rust version 1.91.0",
    "rustc 1.91.0 (d3e2a1f8c 2026-10-27)",
  ],
};

const cache = (key: string): StepSpec => ({
  name: "Run actions/cache@v4",
  lines: () => [
    "##[group]Run actions/cache@v4",
    "with:",
    "  path: ~/.cargo/registry\n  target",
    `  key: ${key}`,
    "##[endgroup]",
    `Cache restored from key: ${key}`,
    "Received 214958080 of 214958080 (100.0%), 102.4 MBs/sec",
  ],
});

const complete: StepSpec = {
  name: "Complete job",
  lines: () => [
    "Cleaning up orphan processes",
    "Evaluate and set job outputs",
    "Uploading runner diagnostic logs",
  ],
};

const compiling = (crates: string[]) =>
  crates.map(
    (k) => `   ${C.boldGreen("Compiling")} ${k} v0.4.0 (/home/runner/work/edge/crates/${k})`,
  );

const testLines = (mods: string[], count: number) => [
  ...compiling(["wire", "api", "raft", "edge"]),
  `    ${C.boldGreen("Finished")} \`test\` profile [unoptimized + debuginfo] target(s) in 48.21s`,
  `     ${C.boldGreen("Running")} unittests src/lib.rs (target/debug/deps/wire-5f1c2a)`,
  "",
  `running ${count} tests`,
  ...mods.map((m) => `test ${m} ... ${C.green("ok")}`),
  "",
  `test result: ${C.green("ok")}. ${count} passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 3.42s`,
];

const EDGE_TESTS = [
  "retry::tests::never_retries_a_post",
  "retry::tests::backoff_is_capped",
  "link::tests::omits_last_on_single_page",
  "link::tests::next_and_last",
  "pagination::tests::empty_per_page_is_default",
  "pagination::tests::per_page_caps_at_100",
  "headers::tests::ratelimit_on_errors",
  "error::tests::basic_error_shape",
  "api::rest::issues::tests::create_accepts_label_objects",
  "api::rest::issues::tests::list_includes_pulls",
  "raft::lease::tests::renews_before_expiry",
  "raft::read::tests::learner_reads_after_commit",
];

const ciSource = `name: CI

on:
  push:
    branches: [main, "release/**"]
  pull_request:
  workflow_dispatch:

jobs:
  rustfmt:
    runs-on: [self-hosted, linux, x64]
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - run: cargo fmt --all --check

  clippy:
    runs-on: [self-hosted, linux, x64]
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - run: cargo clippy --workspace -- -D warnings

  test:
    name: test (ubuntu-latest)
    runs-on: [self-hosted, linux, x64]
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: actions/cache@v4
      - run: cargo test --workspace
      - uses: actions/upload-artifact@v4
        with: { name: test-report, path: target/nextest }

  byte-diff:
    name: byte-diff (GHES 3.22)
    needs: [test]
    runs-on: [self-hosted, linux, x64]
    steps:
      - uses: actions/checkout@v4
      - run: cargo run -p bytediff -- --oracle ghes-3.22 recordings/
`;

const RECORDINGS = [
  "GET /repos/{owner}/{repo}",
  "GET /repos/{owner}/{repo}/issues",
  "GET /repos/{owner}/{repo}/issues/{n}",
  "POST /repos/{owner}/{repo}/issues",
  "GET /repos/{owner}/{repo}/pulls",
  "GET /repos/{owner}/{repo}/pulls/{n}/files",
  "GET /repos/{owner}/{repo}/commits/{ref}/check-runs",
  "GET /user",
  "GET /user/repos",
  "GET /orgs/{org}",
  "GET /repos/{owner}/{repo}/actions/runs",
  "GET /repos/{owner}/{repo}/actions/runs/{id}/jobs",
  "GET /repos/{owner}/{repo}/git/refs/heads",
  "GET /rate_limit",
  "GraphQL repository.issues(first: 30)",
  "GraphQL repository.pullRequests(first: 30)",
];

const edgeCi: WorkflowSpec = {
  file: "ci.yml",
  name: "CI",
  events: ["push", "pull_request", "workflow_dispatch"],
  dispatch: [],
  created: "2025-03-02T09:00:00Z",
  source: ciSource,
  jobs: [
    {
      key: "rustfmt",
      name: "rustfmt",
      runsOn: ["self-hosted", "linux", "x64"],
      steps: [
        setUp(["self-hosted", "linux", "x64"]),
        checkout,
        rustToolchain,
        {
          name: "Run cargo fmt --all --check",
          lines: () => ["##[group]Run cargo fmt --all --check", "##[endgroup]"],
        },
        complete,
      ],
      // The long closure in lib.rs on PR #1's branch is not formatted.
      fails: (c) =>
        c.branch === "mira/idempotent-retry"
          ? {
              step: 3,
              lines: [
                `Diff in /home/runner/work/edge/crates/wire/src/lib.rs at line 22:`,
                "     Client::builder()",
                "         .timeout(Duration::from_secs(10))",
                C.red(
                  "-        .retry(|req, res, n| retry::should_retry(req.method(), res.status(), n))",
                ),
                C.green("+        .retry(|req, res, n| {"),
                C.green("+            retry::should_retry(req.method(), res.status(), n)"),
                C.green("+        })"),
                "         .build()",
                "##[error]Process completed with exit code 1.",
              ],
              annotations: [
                {
                  path: "crates/wire/src/lib.rs",
                  start_line: 25,
                  end_line: 25,
                  start_column: null,
                  end_column: null,
                  annotation_level: "failure",
                  title: "rustfmt",
                  message: "This line is not formatted. Run `cargo fmt --all`.",
                  raw_details: null,
                },
              ],
            }
          : null,
    },
    {
      key: "clippy",
      name: "clippy",
      runsOn: ["self-hosted", "linux", "x64"],
      steps: [
        setUp(["self-hosted", "linux", "x64"]),
        checkout,
        rustToolchain,
        {
          name: "Run cargo clippy --workspace -- -D warnings",
          lines: () => [
            "##[group]Run cargo clippy --workspace -- -D warnings",
            "##[endgroup]",
            `    ${C.boldGreen("Checking")} wire v0.4.0 (/home/runner/work/edge/crates/wire)`,
            `    ${C.boldGreen("Checking")} api v0.4.0 (/home/runner/work/edge/crates/api)`,
            `    ${C.boldGreen("Checking")} edge v0.4.0 (/home/runner/work/edge/workers/edge)`,
            `    ${C.boldGreen("Finished")} \`dev\` profile [unoptimized + debuginfo] target(s) in 31.07s`,
          ],
        },
        complete,
      ],
      fails: (c) =>
        c.branch === "tomas/label-objects"
          ? {
              step: 3,
              lines: [
                `${C.boldRed("error")}${C.bold(": this `map` can be simplified with `filter_map`")}`,
                `  ${C.blue("-->")} crates/api/src/rest/issues.rs:91:17`,
                `   ${C.blue("|")}`,
                `${C.blue("91 |")}             .filter_map(|l| l.as_str().or_else(|| l["name"].as_str()))`,
                `   ${C.blue("|")}                 ${C.boldRed("^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^")}`,
                `   ${C.blue("= note:")} \`-D clippy::manual-filter-map\` implied by \`-D warnings\``,
                `${C.boldRed("error")}: could not compile \`api\` (lib) due to 1 previous error`,
                "##[error]Process completed with exit code 101.",
              ],
              annotations: [
                {
                  path: "crates/api/src/rest/issues.rs",
                  start_line: 91,
                  end_line: 91,
                  start_column: 17,
                  end_column: 75,
                  annotation_level: "failure",
                  title: "clippy::manual-filter-map",
                  message: "this `map` can be simplified with `filter_map`",
                  raw_details: "`-D clippy::manual-filter-map` implied by `-D warnings`",
                },
              ],
            }
          : null,
    },
    {
      key: "test",
      name: "test (ubuntu-latest)",
      runsOn: ["self-hosted", "linux", "x64"],
      artifact: { name: "test-report", size: 482_133 },
      steps: [
        setUp(["self-hosted", "linux", "x64"]),
        checkout,
        rustToolchain,
        cache("cargo-linux-x64-7c1e0a"),
        {
          name: "Run cargo test --workspace",
          lines: () => [
            "##[group]Run cargo test --workspace",
            "##[endgroup]",
            ...testLines(EDGE_TESTS, 312),
          ],
        },
        {
          name: "Run actions/upload-artifact@v4",
          lines: () => [
            "##[group]Run actions/upload-artifact@v4",
            "with:",
            "  name: test-report",
            "  path: target/nextest",
            "##[endgroup]",
            "With the provided path, there will be 14 files uploaded",
            "Artifact test-report has been successfully uploaded! Final size is 482133 bytes.",
          ],
        },
        complete,
      ],
      // A lease test that races the clock on the release branch's slower runner; it
      // passes on a re-run, which is how the run page shows "Re-run failed jobs".
      fails: (c) =>
        c.branch === "release/0.4" && c.attempt === 1
          ? {
              step: 4,
              lines: [
                ...compiling(["wire", "api", "raft", "edge"]),
                "",
                "running 312 tests",
                ...EDGE_TESTS.slice(0, 10).map((m) => `test ${m} ... ${C.green("ok")}`),
                `test raft::lease::tests::renews_before_expiry ... ${C.red("FAILED")}`,
                "",
                "failures:",
                "",
                "---- raft::lease::tests::renews_before_expiry stdout ----",
                "thread 'raft::lease::tests::renews_before_expiry' panicked at crates/raft/src/lease.rs:142:9:",
                "assertion `left == right` failed: lease expired 3 ms before renewal",
                "  left: Expired",
                " right: Held",
                "",
                `test result: ${C.red("FAILED")}. 311 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out`,
                "",
                `${C.boldRed("error")}: test failed, to rerun pass \`-p raft --lib\``,
                "##[error]Process completed with exit code 101.",
              ],
              annotations: [
                {
                  path: "crates/raft/src/lease.rs",
                  start_line: 142,
                  end_line: 142,
                  start_column: 9,
                  end_column: null,
                  annotation_level: "failure",
                  title: "raft::lease::tests::renews_before_expiry",
                  message: "assertion `left == right` failed: lease expired 3 ms before renewal",
                  raw_details: "  left: Expired\n right: Held",
                },
                {
                  path: "crates/raft/src/lease.rs",
                  start_line: 120,
                  end_line: 128,
                  start_column: null,
                  end_column: null,
                  annotation_level: "warning",
                  title: "Slow test",
                  message:
                    "This test took 4.8 s; tests that sleep on the wall clock flake on loaded runners.",
                  raw_details: null,
                },
              ],
            }
          : null,
    },
    {
      key: "byte-diff",
      name: "byte-diff (GHES 3.22)",
      needs: ["test"],
      runsOn: ["self-hosted", "linux", "x64"],
      steps: [
        setUp(["self-hosted", "linux", "x64"]),
        checkout,
        {
          name: "Run cargo run -p bytediff -- --oracle ghes-3.22 recordings/",
          lines: () => [
            "##[group]Run cargo run -p bytediff -- --oracle ghes-3.22 recordings/",
            "##[endgroup]",
            `    ${C.boldGreen("Finished")} \`release\` profile [optimized] target(s) in 1m 12s`,
            `     ${C.boldGreen("Running")} \`target/release/bytediff --oracle ghes-3.22 recordings/\``,
            C.cyan("Loaded 1,204 recorded responses from recordings/ghes-3.22"),
            ...RECORDINGS.flatMap((r, i) => [
              `##[group]${r}`,
              `  replaying ${60 + ((i * 37) % 40)} requests`,
              `  status, headers, and body match ${C.dim("(ignoring date, x-github-request-id, etag)")}`,
              "##[endgroup]",
              `${C.green("✓")} ${r}`,
            ]),
            "",
            C.boldGreen("1204 of 1204 responses byte-identical"),
          ],
        },
        complete,
      ],
    },
  ],
};

const deploySource = `name: Deploy

on:
  push:
    branches: [main]
  workflow_dispatch:
    inputs:
      environment:
        description: Where to deploy
        type: environment
        required: true
        default: staging
      version:
        description: Release tag or commit to deploy
        type: string
        required: true
        default: main
      region:
        description: Which Cloudflare regions to update first
        type: choice
        options: [all, weur, enam, apac]
        default: all
      dry_run:
        description: Build and plan only, change nothing
        type: boolean
        default: false

jobs:
  build:
    runs-on: [self-hosted, linux, x64]
    steps:
      - uses: actions/checkout@v4
      - run: cargo build --release -p edge --target wasm32-unknown-unknown
      - uses: actions/upload-artifact@v4
        with: { name: edge-worker, path: target/edge.wasm }

  staging:
    needs: [build]
    environment: staging
    runs-on: [self-hosted, linux, x64]
    steps:
      - run: graftyard site-day plan --env staging

  production:
    needs: [staging]
    if: github.event_name == 'push' || inputs.environment == 'production'
    environment: production
    runs-on: [self-hosted, linux, x64]
    steps:
      - run: graftyard site-day apply --env production
`;

const siteDay = (env: string): StepSpec => ({
  name: `Run graftyard site-day apply --env ${env}`,
  lines: (c) => [
    `##[group]Run graftyard site-day apply --env ${env}${c.inputs.dry_run === true ? " --dry-run" : ""}`,
    "env:",
    "  GRAFTYARD_TOKEN: ***",
    "##[endgroup]",
    C.cyan(`Site day for ${env}, region ${c.inputs.region ?? "all"}`),
    `Version: ${c.inputs.version ?? c.sha.slice(0, 7)}`,
    "Reading the release log... entry 418 (ControllerPointer)",
    "Signing the new site-script pointer",
    ...["weur-1", "enam-1", "apac-1"].flatMap((site) => [
      `##[group]${site}`,
      `  drain members on ${site}`,
      `  upload site script (1.84 MB, sha256:${c.sha.slice(0, 12)})`,
      `  readiness probe ${C.green("200")} in 41 ms`,
      "##[endgroup]",
      `${C.green("✓")} ${site} serving the new version`,
    ]),
    c.inputs.dry_run === true
      ? C.yellow("Dry run: nothing changed")
      : C.boldGreen(`Deployed to ${env}`),
  ],
});

const edgeDeploy: WorkflowSpec = {
  file: "deploy.yml",
  // `version` must name a branch, a tag, or a commit sha.
  checkInputs: (full, inputs) => {
    const v = String(inputs.version ?? "");
    const known =
      [...(branches[full] ?? []), ...(tags[full] ?? [])].includes(v) || /^[0-9a-f]{7,40}$/.test(v);
    return known ? null : { field: "inputs.version", code: "invalid" };
  },
  name: "Deploy",
  events: ["push", "workflow_dispatch"],
  created: "2025-04-11T12:00:00Z",
  source: deploySource,
  dispatch: [
    {
      name: "environment",
      input: {
        type: "environment",
        description: "Where to deploy",
        required: true,
        default: "staging",
      },
    },
    {
      name: "version",
      input: {
        type: "string",
        description: "Release tag or commit to deploy",
        required: true,
        default: "main",
      },
    },
    {
      name: "region",
      input: {
        type: "choice",
        description: "Which Cloudflare regions to update first",
        required: false,
        default: "all",
        options: ["all", "weur", "enam", "apac"],
      },
    },
    {
      name: "dry_run",
      input: {
        type: "boolean",
        description: "Build and plan only, change nothing",
        required: false,
        default: false,
      },
    },
  ],
  jobs: [
    {
      key: "build",
      name: "build",
      runsOn: ["self-hosted", "linux", "x64"],
      artifact: { name: "edge-worker", size: 1_929_412 },
      steps: [
        setUp(["self-hosted", "linux", "x64"]),
        checkout,
        {
          name: "Run cargo build --release -p edge --target wasm32-unknown-unknown",
          lines: () => [
            "##[group]Run cargo build --release -p edge --target wasm32-unknown-unknown",
            "##[endgroup]",
            ...compiling(["wire", "api", "raft", "edge"]),
            `    ${C.boldGreen("Finished")} \`release\` profile [optimized] target(s) in 2m 03s`,
            "edge.wasm: 1.84 MB (612 KB gzip)",
          ],
        },
        {
          name: "Run actions/upload-artifact@v4",
          lines: () => [
            "##[group]Run actions/upload-artifact@v4",
            "##[endgroup]",
            "Artifact edge-worker has been successfully uploaded! Final size is 1929412 bytes.",
          ],
        },
        complete,
      ],
    },
    {
      key: "staging",
      name: "staging",
      needs: ["build"],
      environment: "staging",
      runsOn: ["self-hosted", "linux", "x64"],
      steps: [setUp(["self-hosted", "linux", "x64"]), siteDay("staging"), complete],
    },
    {
      key: "production",
      name: "production",
      needs: ["staging"],
      when: (c) => c.event !== "workflow_dispatch" || c.inputs.environment === "production",
      environment: "production",
      runsOn: ["self-hosted", "linux", "x64"],
      steps: [setUp(["self-hosted", "linux", "x64"]), siteDay("production"), complete],
    },
  ],
};

const chaosSource = `name: Chaos

on:
  schedule:
    - cron: "17 3 * * *"
  workflow_dispatch:
    inputs:
      duration:
        description: How long to keep each partition
        type: choice
        options: [5m, 15m, 1h]
        default: 15m
      sites:
        description: Comma-separated sites to partition
        type: string
        required: false

jobs:
  partition:
    name: partition (3 sites)
    runs-on: [self-hosted, linux, x64]
    steps:
      - run: ./scripts/chaos.sh --duration \${{ inputs.duration || '15m' }}
      - uses: actions/upload-artifact@v4
        with: { name: chaos-report, path: out/ }
  report:
    needs: [partition]
    runs-on: [self-hosted, linux, x64]
    steps:
      - run: ./scripts/chaos-report.sh out/
`;

const edgeChaos: WorkflowSpec = {
  file: "chaos.yml",
  name: "Chaos",
  events: ["schedule", "workflow_dispatch"],
  created: "2025-06-01T08:00:00Z",
  source: chaosSource,
  dispatch: [
    {
      name: "duration",
      input: {
        type: "choice",
        description: "How long to keep each partition",
        required: true,
        default: "15m",
        options: ["5m", "15m", "1h"],
      },
    },
    {
      name: "sites",
      input: { type: "string", description: "Comma-separated sites to partition", required: false },
    },
  ],
  jobs: [
    {
      key: "partition",
      name: "partition (3 sites)",
      runsOn: ["self-hosted", "linux", "x64"],
      artifact: { name: "chaos-report", size: 3_214_887 },
      steps: [
        setUp(["self-hosted", "linux", "x64"]),
        checkout,
        {
          name: "Run ./scripts/chaos.sh",
          lines: (c) => [
            `##[group]Run ./scripts/chaos.sh --duration ${c.inputs.duration ?? "15m"}`,
            "##[endgroup]",
            ...["weur-1", "enam-1", "apac-1"].flatMap((s) => [
              C.yellow(`partition ${s} from its peers`),
              "  writes acknowledged: 18,204; lost: 0",
              "  leader moved in 1.9 s",
              C.green(`heal ${s}`),
            ]),
            C.boldGreen("No acknowledged write was lost"),
          ],
        },
        {
          name: "Run actions/upload-artifact@v4",
          lines: () => [
            "##[group]Run actions/upload-artifact@v4",
            "##[endgroup]",
            "Artifact chaos-report has been successfully uploaded! Final size is 3214887 bytes.",
          ],
        },
        complete,
      ],
    },
    {
      key: "report",
      name: "report",
      needs: ["partition"],
      runsOn: ["self-hosted", "linux", "x64"],
      steps: [
        setUp(["self-hosted", "linux", "x64"]),
        {
          name: "Run ./scripts/chaos-report.sh out/",
          lines: () => [
            "##[group]Run ./scripts/chaos-report.sh out/",
            "##[endgroup]",
            "Summary: 3 partitions, 0 lost writes, p99 failover 2.1 s",
          ],
        },
        complete,
      ],
    },
  ],
};

const edgeGpu: WorkflowSpec = {
  file: "bench-gpu.yml",
  name: "Bench (GPU)",
  events: ["pull_request"],
  created: "2026-10-20T08:00:00Z",
  source: `name: Bench (GPU)

on:
  pull_request:
    paths: ["crates/raft/**"]

jobs:
  bench:
    runs-on: [self-hosted, gpu]
    steps:
      - uses: actions/checkout@v4
      - run: cargo bench -p raft
`,
  dispatch: null,
  jobs: [
    {
      key: "bench",
      name: "bench",
      runsOn: ["self-hosted", "gpu"],
      steps: [
        setUp(["self-hosted", "gpu"]),
        checkout,
        {
          name: "Run cargo bench -p raft",
          lines: () => [
            "##[group]Run cargo bench -p raft",
            "##[endgroup]",
            "read_index  time: [41.2 µs 41.9 µs 42.7 µs]",
          ],
        },
        complete,
      ],
    },
  ],
};

const edgeStale: WorkflowSpec = {
  file: "stale.yml",
  name: "Stale issues",
  events: ["schedule"],
  state: "disabled_manually",
  created: "2025-05-04T08:00:00Z",
  source: `name: Stale issues

on:
  schedule:
    - cron: "0 6 * * 1"

jobs:
  stale:
    runs-on: [self-hosted, linux, x64]
    steps:
      - uses: actions/stale@v9
        with: { days-before-stale: 60 }
`,
  dispatch: null,
  jobs: [
    {
      key: "stale",
      name: "stale",
      runsOn: ["self-hosted", "linux", "x64"],
      steps: [
        setUp(["self-hosted", "linux", "x64"]),
        {
          name: "Run actions/stale@v9",
          lines: () => [
            "##[group]Run actions/stale@v9",
            "##[endgroup]",
            "Processed 31 issues, marked 0 stale",
          ],
        },
        complete,
      ],
    },
  ],
};

const simpleCi = (repoName: string, jobs: [string, string, string[]][]): WorkflowSpec => ({
  file: "ci.yml",
  name: "CI",
  events: ["push", "pull_request"],
  created: "2025-03-02T09:00:00Z",
  dispatch: null,
  source: `name: CI\n\non: [push, pull_request]\n\njobs:\n${jobs
    .map(
      ([key, cmd, needs]) =>
        `  ${key}:\n${needs.length ? `    needs: [${needs.join(", ")}]\n` : ""}    runs-on: [self-hosted, linux, x64]\n    steps:\n      - uses: actions/checkout@v4\n      - run: ${cmd}\n`,
    )
    .join("\n")}`,
  jobs: jobs.map(([key, cmd, needs]) => ({
    key,
    name: key,
    needs,
    runsOn: ["self-hosted", "linux", "x64"],
    steps: [
      setUp(["self-hosted", "linux", "x64"]),
      checkout,
      {
        name: `Run ${cmd}`,
        lines: () => [
          `##[group]Run ${cmd}`,
          "##[endgroup]",
          ...(repoName === "web"
            ? [
                "> graftyard-web@0.0.0 check",
                "> npm run format:check && npm run typecheck && npm run build",
                C.green("All matched files use the correct format."),
                "nav: every target is one of 41 routes",
                C.dim("vite v8.0.0 building for production..."),
                C.green("✓ built in 9.41s"),
              ]
            : testLines(
                [
                  "log::tests::append_then_read",
                  "lease::tests::renews_before_expiry",
                  "transfer::tests::hands_off_leadership",
                ],
                88,
              )),
        ],
      },
      complete,
    ],
  })),
});

export const workflowSpecs: Record<string, WorkflowSpec[]> = {
  "graftyard/edge": [edgeCi, edgeDeploy, edgeChaos, edgeGpu, edgeStale],
  "graftyard/raft": [
    simpleCi("raft", [
      ["test", "cargo test", []],
      ["jepsen", "./jepsen/run.sh --time-limit 300", ["test"]],
    ]),
  ],
  "graftyard/web": [
    simpleCi("web", [
      ["check", "npm run check", []],
      ["e2e", "npm run e2e", ["check"]],
    ]),
  ],
};
