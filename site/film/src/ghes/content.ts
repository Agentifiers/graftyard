// The GHES clients loop's own strings, every one from the record (README.md, tests/RESULTS.md, research/clients.md):
// gh reads installed_version from GET /api/v3/meta; Graftyard answers 3.22.0 (crates/api/src/rest.rs meta()), and gh
// pretty-prints the reply with its keys in the order the host sent them. Unmodified clients then passed: gh 2.102.1
// issue, pr and release commands; git-lfs 3.8.0 push, clone, pull and lock; clients.sh's 29 rows on git 2.39.5,
// 2.51.0 and 2.56.0.

export const TERMINAL = {
  title: 'Graftyard',
  prompt: '$',
  command: 'gh api meta --hostname git.example',
  reply: ['{', '  "installed_version": "3.22.0",', '  "verifiable_password_authentication": false', '}'],
  /** The reply's line and the characters set in the accent: the version the clients read. */
  version: { row: 1, text: '"3.22.0"' },
};

export type Client = { name: string; rows: string[] };

/** Left to right as the cards stand. git's rows are its versions; each counts clients.sh's rows. */
export const CLIENTS: Client[] = [
  { name: 'gh 2.102.1', rows: ['issue', 'pr', 'release'] },
  { name: 'git-lfs 3.8.0', rows: ['push', 'clone', 'pull', 'lock'] },
  { name: 'git', rows: ['2.39.5', '2.51.0', '2.56.0'] },
];

/** clients.sh rows per git version, every one passing in every run. */
export const GIT_ROWS = 29;
