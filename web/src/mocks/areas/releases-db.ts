import type { Schemas } from "@/api/client";
import {
  HOST,
  branchSha,
  edge,
  findRepo,
  id,
  raft,
  sha,
  tags,
  users,
  type Repository,
  type SimpleUser,
} from "../world";

// Releases, their assets, and reactions, per repository. Records are typed by the GHES
// schema they are served as. Tags created by publishing a release live here too, so the
// tag picker and the source archive links see them.

export type Release = Schemas["release"];
export type ReleaseAsset = Schemas["release-asset"];
export type Reaction = Schemas["reaction"];
type ReactionContent = Reaction["content"];

type RepoReleases = {
  repo: Repository;
  releases: Release[];
  // The release a write marked with make_latest=true. GHES falls back to the newest
  // published full release when this is unset or no longer eligible.
  latestId: number | null;
  reactions: Map<number, Reaction[]>;
  createdTags: string[];
};

const bytes = new Map<number, Uint8Array<ArrayBuffer>>();

const digest = (seed: string) => `sha256:${sha(seed)}${sha(`${seed}:2`).slice(0, 24)}`;

export function asset(
  repo: Repository,
  tag: string,
  s: { name: string; size: number; downloads: number; type: string; at: string; by?: SimpleUser },
): ReleaseAsset {
  const n = id();
  return {
    url: `${repo.url}/releases/assets/${n}`,
    browser_download_url: `${repo.html_url}/releases/download/${tag}/${encodeURIComponent(s.name)}`,
    id: n,
    node_id: `RA_${sha(`${repo.full_name}:${tag}:${s.name}`).slice(0, 12)}`,
    name: s.name,
    label: null,
    state: "uploaded",
    content_type: s.type,
    size: s.size,
    digest: digest(`${repo.full_name}:${tag}:${s.name}`),
    download_count: s.downloads,
    created_at: s.at,
    updated_at: s.at,
    uploader: s.by ?? users.mira,
  };
}

type ReleaseSeed = {
  tag: string;
  target?: string;
  name: string;
  body: string;
  author: SimpleUser;
  created: string;
  draft?: boolean;
  prerelease?: boolean;
  assets?: Omit<Parameters<typeof asset>[2], "at">[];
};

const rollup = (url: string, list: Reaction[]): Schemas["reaction-rollup"] => {
  const count = (c: ReactionContent) => list.filter((r) => r.content === c).length;
  return {
    url,
    total_count: list.length,
    "+1": count("+1"),
    "-1": count("-1"),
    laugh: count("laugh"),
    confused: count("confused"),
    heart: count("heart"),
    hooray: count("hooray"),
    eyes: count("eyes"),
    rocket: count("rocket"),
  };
};

export function release(repo: Repository, s: ReleaseSeed): Release {
  const n = id();
  const url = `${repo.url}/releases/${n}`;
  const published = !s.draft;
  return {
    url,
    html_url: `${repo.html_url}/releases/tag/${s.tag}`,
    assets_url: `${url}/assets`,
    upload_url: `${HOST}/api/uploads/repos/${repo.full_name}/releases/${n}/assets{?name,label}`,
    tarball_url: published ? `${repo.url}/tarball/${s.tag}` : null,
    zipball_url: published ? `${repo.url}/zipball/${s.tag}` : null,
    id: n,
    node_id: `RE_${sha(`${repo.full_name}:release:${s.tag}`).slice(0, 12)}`,
    tag_name: s.tag,
    target_commitish: s.target ?? "main",
    name: s.name,
    body: s.body,
    draft: Boolean(s.draft),
    prerelease: Boolean(s.prerelease),
    immutable: false,
    created_at: s.created,
    published_at: published ? s.created : null,
    updated_at: s.created,
    author: s.author,
    assets: (s.assets ?? []).map((a) => asset(repo, s.tag, { ...a, at: s.created })),
    mentions_count: 0,
  };
}

const LINUX = "application/gzip";
const MIB = 1024 * 1024;

const edgeSeeds: ReleaseSeed[] = [
  {
    tag: "v0.1.0",
    name: "v0.1.0: the read path",
    author: users.mira,
    created: "2025-06-02T10:00:00Z",
    body: "The first tagged edge Worker. It serves the GHES 3.22 read paths for repositories, issues, and pull requests from the fold.\n\n### Highlights\n\n- `GET /repos/{owner}/{repo}` and `GET /repos/{owner}/{repo}/issues` answer byte-identical to GHES for the fixtures in `crates/wire/tests`.\n- Pagination follows GHES: `per_page` defaults to 30 and caps at 100.\n\n### Known gaps\n\n- No writes yet. Every `POST` answers `501`.",
    assets: [
      {
        name: "graftyard-edge-v0.1.0-x86_64-unknown-linux-gnu.tar.gz",
        size: Math.round(6.2 * MIB),
        downloads: 41,
        type: LINUX,
      },
      { name: "SHA256SUMS.txt", size: 210, downloads: 18, type: "text/plain" },
    ],
  },
  {
    tag: "v0.2.0",
    name: "v0.2.0: writes behind the quorum",
    author: users.devi,
    created: "2025-09-15T14:30:00Z",
    body: "Writes land. A write is acknowledged only after the quorum rule holds.\n\n### Added\n\n- Issue and comment writes (`POST /repos/{owner}/{repo}/issues`, `POST .../comments`).\n- `Link` headers on every paginated list.\n\n### Fixed\n\n- Label colours keep GHES's hex-without-hash form.",
    assets: [
      {
        name: "graftyard-edge-v0.2.0-x86_64-unknown-linux-gnu.tar.gz",
        size: Math.round(7.1 * MIB),
        downloads: 133,
        type: LINUX,
      },
      {
        name: "graftyard-edge-v0.2.0-aarch64-unknown-linux-gnu.tar.gz",
        size: Math.round(6.8 * MIB),
        downloads: 52,
        type: LINUX,
      },
      { name: "SHA256SUMS.txt", size: 336, downloads: 40, type: "text/plain" },
    ],
  },
  {
    tag: "v0.3.0",
    name: "v0.3.0: pull requests and checks",
    author: users.devi,
    created: "2026-10-30T16:00:00Z",
    body: "Pull requests, reviews, and check runs, wire-compatible with GHES 3.22.\n\n## What's Changed\n\n* Send rate limit headers on every error response by @devi in https://ghes.graftyard.test/graftyard/edge/pull/5\n* Pull request files and the diff renderer by @lin\n* Check runs list resolves a branch or tag `ref` by @tomas\n\n## Upgrade notes\n\nRun `graftyard migrate --to 0.3` before you swap the script. The new fold adds a `checks` column.\n\n**Full Changelog**: https://ghes.graftyard.test/graftyard/edge/compare/v0.2.0...v0.3.0",
    assets: [
      {
        name: "graftyard-edge-v0.3.0-x86_64-unknown-linux-gnu.tar.gz",
        size: Math.round(8.4 * MIB),
        downloads: 212,
        type: LINUX,
        by: users.devi,
      },
      {
        name: "graftyard-edge-v0.3.0-aarch64-unknown-linux-gnu.tar.gz",
        size: Math.round(7.9 * MIB),
        downloads: 96,
        type: LINUX,
        by: users.devi,
      },
      {
        name: "graftyard_edge_bg.wasm",
        size: Math.round(2.1 * MIB),
        downloads: 64,
        type: "application/wasm",
        by: users.devi,
      },
      {
        name: "ghes-3.22-wire-diff.json",
        size: 48_213,
        downloads: 23,
        type: "application/json",
        by: users.devi,
      },
      { name: "SHA256SUMS.txt", size: 452, downloads: 71, type: "text/plain", by: users.devi },
    ],
  },
  {
    tag: "v0.4.0-rc.1",
    name: "v0.4.0-rc.1",
    author: users.mira,
    created: "2026-11-06T12:00:00Z",
    prerelease: true,
    body: "Release candidate for 0.4. Idempotent retries at the edge client and learner reads.\n\n**Not for production.** The learner read-index is behind `GRAFTYARD_LEARNER_READS=1`.",
    assets: [
      {
        name: "graftyard-edge-v0.4.0-rc.1-x86_64-unknown-linux-gnu.tar.gz",
        size: Math.round(8.6 * MIB),
        downloads: 9,
        type: LINUX,
      },
    ],
  },
  {
    tag: "v0.4.0",
    target: "release/0.4",
    name: "v0.4.0",
    author: users.mira,
    created: "2026-11-08T09:00:00Z",
    draft: true,
    body: "Draft. Fill in after rc.2.\n\n- Idempotent reads retry at most twice with full jitter.\n- Learner read-index for hot issue lists.",
  },
];

const raftSeeds: ReleaseSeed[] = [
  {
    tag: "v0.1.0",
    name: "raft v0.1.0",
    author: users.lin,
    created: "2026-09-01T08:00:00Z",
    body: "Log, lease, transfer, and learners for each group.\n\n- Joint consensus is not in this release.",
    assets: [
      { name: "raft-bench-results.json", size: 18_920, downloads: 7, type: "application/json" },
    ],
  },
];

function seed(repo: Repository, seeds: ReleaseSeed[]): RepoReleases {
  return {
    repo,
    releases: seeds.map((s) => release(repo, s)),
    latestId: null,
    reactions: new Map(),
    createdTags: [],
  };
}

const store = new Map<string, RepoReleases>([
  ["graftyard/edge", seed(edge, edgeSeeds)],
  ["graftyard/raft", seed(raft, raftSeeds)],
]);

let reactionId = 9000;
function react(
  r: RepoReleases,
  tag: string,
  who: SimpleUser,
  content: ReactionContent,
  at: string,
) {
  const rel = r.releases.find((x) => x.tag_name === tag)!;
  const list = r.reactions.get(rel.id) ?? [];
  list.push({
    id: reactionId++,
    node_id: `REA_${sha(`${rel.id}:${who.login}:${content}`).slice(0, 12)}`,
    user: who,
    content,
    created_at: at,
  });
  r.reactions.set(rel.id, list);
}
{
  const e = store.get("graftyard/edge")!;
  react(e, "v0.3.0", users.tomas, "hooray", "2026-10-30T17:00:00Z");
  react(e, "v0.3.0", users.lin, "hooray", "2026-10-30T17:10:00Z");
  react(e, "v0.3.0", users.lin, "rocket", "2026-10-30T17:10:00Z");
  react(e, "v0.3.0", users.devi, "rocket", "2026-10-30T18:00:00Z");
  react(e, "v0.3.0", users.tomas, "heart", "2026-10-31T08:00:00Z");
  react(e, "v0.4.0-rc.1", users.devi, "eyes", "2026-11-06T13:00:00Z");
}

// ---- Store API the handlers use ----

function state(owner: string, name: string): RepoReleases | undefined {
  const key = `${owner}/${name}`.toLowerCase();
  let s = store.get(key);
  const repo = findRepo(owner, name);
  if (!s && repo) store.set(key, (s = seed(repo, [])));
  return s;
}

const isFull = (r: Release) => !r.draft && !r.prerelease;
const newestFirst = (a: Release, b: Release) => b.created_at.localeCompare(a.created_at);

function latest(s: RepoReleases): Release | undefined {
  const marked = s.releases.find((r) => r.id === s.latestId && isFull(r));
  return marked ?? s.releases.filter(isFull).sort(newestFirst)[0];
}

function withReactions(s: RepoReleases, r: Release): Release {
  return { ...r, reactions: rollup(`${r.url}/reactions`, s.reactions.get(r.id) ?? []) };
}

export const releasesDb = {
  state,
  list: (s: RepoReleases) => [...s.releases].sort(newestFirst).map((r) => withReactions(s, r)),
  get: (s: RepoReleases, releaseId: number) => {
    const r = s.releases.find((x) => x.id === releaseId);
    return r && withReactions(s, r);
  },
  byTag: (s: RepoReleases, tag: string) => {
    const r = s.releases.find((x) => x.tag_name === tag && !x.draft);
    return r && withReactions(s, r);
  },
  latest: (s: RepoReleases) => {
    const r = latest(s);
    return r && withReactions(s, r);
  },
  record: (s: RepoReleases, releaseId: number) => s.releases.find((x) => x.id === releaseId),

  // Every tag the repository has: the shared world's, plus those publishing a release made.
  tags: (s: RepoReleases) => [...(tags[s.repo.full_name] ?? []), ...s.createdTags],
  tagSha: (s: RepoReleases, tag: string) => branchSha(s.repo.full_name, `tag:${tag}`),

  create(
    s: RepoReleases,
    input: {
      tag_name: string;
      target_commitish?: string;
      name?: string;
      body?: string;
      draft?: boolean;
      prerelease?: boolean;
      make_latest?: "true" | "false" | "legacy";
    },
  ): Release {
    const now = new Date().toISOString();
    const r = release(s.repo, {
      tag: input.tag_name,
      target: input.target_commitish,
      name: input.name || input.tag_name,
      body: input.body ?? "",
      author: users.mira,
      created: now,
      draft: input.draft,
      prerelease: input.prerelease,
    });
    s.releases.push(r);
    this.publishTag(s, r);
    if (input.make_latest !== "false" && isFull(r)) s.latestId = r.id;
    return withReactions(s, r);
  },

  update(
    s: RepoReleases,
    r: Release,
    input: {
      tag_name?: string;
      target_commitish?: string;
      name?: string;
      body?: string;
      draft?: boolean;
      prerelease?: boolean;
      make_latest?: "true" | "false" | "legacy";
    },
  ): Release {
    const now = new Date().toISOString();
    if (input.tag_name !== undefined) r.tag_name = input.tag_name;
    if (input.target_commitish !== undefined) r.target_commitish = input.target_commitish;
    if (input.name !== undefined) r.name = input.name;
    if (input.body !== undefined) r.body = input.body;
    if (input.prerelease !== undefined) r.prerelease = input.prerelease;
    if (input.draft !== undefined) {
      if (r.draft && !input.draft) r.published_at = now;
      if (input.draft) r.published_at = null;
      r.draft = input.draft;
    }
    r.html_url = `${s.repo.html_url}/releases/tag/${r.tag_name}`;
    r.tarball_url = r.draft ? null : `${s.repo.url}/tarball/${r.tag_name}`;
    r.zipball_url = r.draft ? null : `${s.repo.url}/zipball/${r.tag_name}`;
    r.updated_at = now;
    this.publishTag(s, r);
    if (input.make_latest === "true" && isFull(r)) s.latestId = r.id;
    if (input.make_latest === "false" && s.latestId === r.id) s.latestId = null;
    return withReactions(s, r);
  },

  // Publishing a release whose tag does not exist creates the tag from its target.
  publishTag(s: RepoReleases, r: Release) {
    if (!r.draft && !this.tags(s).includes(r.tag_name)) s.createdTags.push(r.tag_name);
  },

  remove(s: RepoReleases, releaseId: number) {
    s.releases = s.releases.filter((r) => r.id !== releaseId);
    s.reactions.delete(releaseId);
    if (s.latestId === releaseId) s.latestId = null;
  },

  findAsset(s: RepoReleases, assetId: number) {
    for (const r of s.releases) {
      const a = r.assets.find((x) => x.id === assetId);
      if (a) return { release: r, asset: a };
    }
    return undefined;
  },

  addAsset(
    s: RepoReleases,
    r: Release,
    input: { name: string; label?: string; type: string; data: Uint8Array<ArrayBuffer> },
  ) {
    const now = new Date().toISOString();
    const a = asset(s.repo, r.tag_name, {
      name: input.name,
      size: input.data.byteLength,
      downloads: 0,
      type: input.type,
      at: now,
    });
    a.label = input.label ?? null;
    bytes.set(a.id, input.data);
    r.assets.push(a);
    return a;
  },

  // Seeded assets have no uploaded bytes; their download is zero-filled to the listed size.
  assetBytes: (a: ReleaseAsset) => bytes.get(a.id) ?? new Uint8Array(a.size),

  removeAsset(s: RepoReleases, assetId: number) {
    const found = this.findAsset(s, assetId);
    if (!found) return false;
    found.release.assets = found.release.assets.filter((a) => a.id !== assetId);
    bytes.delete(assetId);
    return true;
  },

  reactions: (s: RepoReleases, releaseId: number) => s.reactions.get(releaseId) ?? [],

  // GHES answers 200 with the existing reaction when the viewer already left this one.
  react(s: RepoReleases, releaseId: number, content: ReactionContent) {
    const list = s.reactions.get(releaseId) ?? [];
    const existing = list.find((r) => r.user?.login === users.mira.login && r.content === content);
    if (existing) return { reaction: existing, created: false };
    const reaction: Reaction = {
      id: reactionId++,
      node_id: `REA_${sha(`${releaseId}:mira:${content}:${reactionId}`).slice(0, 12)}`,
      user: users.mira,
      content,
      created_at: new Date().toISOString(),
    };
    s.reactions.set(releaseId, [...list, reaction]);
    return { reaction, created: true };
  },

  unreact(s: RepoReleases, releaseId: number, reactionIdToDelete: number) {
    const list = s.reactions.get(releaseId) ?? [];
    s.reactions.set(
      releaseId,
      list.filter((r) => r.id !== reactionIdToDelete),
    );
  },

  // The notes GHES would write: merged pull requests since the previous tag, by author.
  generateNotes(
    s: RepoReleases,
    pulls: { title: string; user: SimpleUser; html_url: string; merged_at: string | null }[],
    input: { tag_name: string; previous_tag_name?: string },
  ): Schemas["release-notes-content"] {
    const previous =
      input.previous_tag_name ||
      latest(s)?.tag_name ||
      [...this.tags(s)].filter((t) => t !== input.tag_name).at(-1);
    const merged = pulls.filter((p) => p.merged_at);
    const lines = merged.map((p) => `* ${p.title} by @${p.user.login} in ${p.html_url}`);
    const earlier = new Set(
      s.releases.filter((r) => r.tag_name !== input.tag_name).map((r) => r.author.login),
    );
    const newcomers = [...new Set(merged.map((p) => p.user.login))].filter((l) => !earlier.has(l));
    const range = previous ? `${previous}...${input.tag_name}` : input.tag_name;
    const body = [
      "## What's Changed",
      "",
      ...(lines.length ? lines : ["No pull requests were merged in this range."]),
      ...(newcomers.length
        ? [
            "",
            "## New Contributors",
            "",
            ...newcomers.map((l) => `* @${l} made their first contribution`),
          ]
        : []),
      "",
      `**Full Changelog**: ${s.repo.html_url}/compare/${range}`,
    ].join("\n");
    return { name: input.tag_name, body };
  },
};
