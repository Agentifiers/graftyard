# What gh sends, and the oracle its replies are checked against

The `*.json` files here are every GraphQL document that gh 2.102.1 (`/usr/bin/gh`, sha256
`30bfdd4abead53dab4452ec507dd3641480dd168ad3b39edd86e2338191a77eb`) sent for the `gh pr` and `gh issue` commands that
`build/BUILD-PLAN.md`, M2-WP2, names. `rest.txt` lists its REST requests. `capture.py` wrote them from the capture log.

## How they were captured

1. A local TLS server listened on `127.0.0.1:443` as `ghes.graftyard.test`, which gh classifies as GHES
   (`design/SHARED.md`, "Wire compatibility"). It logged every request and answered GraphQL by executing it with
   graphql-core on the pinned GHES 3.22 SDL, so gh received only schema-valid replies.
2. gh ran with an empty `HOME`, its own `GH_CONFIG_DIR`, `GH_HOST=ghes.graftyard.test`, and a fake
   `GH_ENTERPRISE_TOKEN`. No GitHub credential was used, and nothing went to github.com.
3. These 24 commands each ran once, with gh's `__type` cache cleared first, and all exited 0:

   ```
   pr list; pr list --state all --limit 5; pr list --label bug; pr list --json number,title,state,headRefName,isDraft,url
   pr view 1; pr view 1 --comments; pr view 1 --json number,title,state,body,mergeable,statusCheckRollup,reviewDecision,headRefOid
   pr create -H f -B main -t T -b B; pr create ... --draft
   pr checks 1; pr checks 1 --required; pr checks 1 --json name,state,bucket,link
   pr merge 1 --merge; pr merge 1 --squash; pr merge 1 --rebase --delete-branch
   issue list; issue list --label bug --state all; issue list --json number,title,state,labels,updatedAt
   issue view 1; issue view 1 --comments; issue view 1 --json number,title,body,state,labels,comments
   issue create -t T -b B; issue create -t T -b B -l bug; issue comment 1 -b hi
   ```

   That made 48 requests: 46 GraphQL posts holding 29 distinct documents, plus `GET /api/v3/meta` and `DELETE
   /api/v3/repos/{owner}/{repo}/git/refs/heads%2Ff`. The variable values `I_1`, `PR_1`, and `R_1` are the capture
   server's node ids. The tests replace them with the ids the handlers produce.

`--author`, `--assignee`, `--draft`, and `--base` on `pr list` and `issue list` were also run. They send only
`PullRequestSearch`, `IssueList`, and `PullRequestList` documents that are already here, with other variables.

M2-WP4 added `RepositoryReleaseByTag-1.json`, `RepositoryReleaseList-1.json`, and `Release_fields-1.json`, the three
documents gh sent for the `gh release` commands in `../lfs/README.md`, which also lists that run's REST and LFS
requests.

## The oracle

`decisions/DECISIONS.md`, "L0-11's oracle is the pinned GHES 3.22 descriptions and github.com", sets the oracle. These
are the files:

- `ghes-3.22.json` from `github/rest-api-description` at `755ee33b4a81d51f6f07a902cce26cbec3b822f2`, the commit
  `scripts/census.py` pins. Its sha256 is `b5723858ee439961d93975514079f3b0302485f4b35a00613e88ea8ca8085960`.
- `https://docs.github.com/public/ghes-3.22/schema.docs-enterprise.graphql`, sha256
  `fe9c21b8fa21a2991cbbb9cfa73a13b8f2642691f64d24184b324347b9fe1391`.

## Rerun the checks

```
uv venv /tmp/ghes/.venv && uv pip install --python /tmp/ghes/.venv/bin/python graphql-core jsonschema
mkdir -p /tmp/replies
GRAFTYARD_REPLIES=/tmp/replies cargo test -p graftyard-api   # surfaces.rs and blobs.rs write replies
/tmp/ghes/.venv/bin/python crates/api/gh/validate.py /tmp/ghes/ghes-3.22.json /tmp/ghes/schema.docs-enterprise.graphql /tmp/replies
cargo run -p graftyard-api --example gen_schema -- /tmp/ghes/schema.docs-enterprise.graphql > crates/api/src/schema.txt
```

`src/schema.txt` is generated from the pinned SDL by `examples/gen_schema.rs`, and its first line names the SDL's
sha256. The executor takes from it which fields each type has, which fragments apply, and what `__type` answers, and
it checks every value a resolver returns against the field's type.

`validate.py` checks each GraphQL document against the SDL, then re-executes it over the reply's own values, so every
type, nullability, enum value, and selected key is checked, and an extra key fails. It checks each REST body with
jsonschema against the operation's response schema, and each LFS body against git-lfs's schema in `../lfs/schemas/`.

To drive the handlers with gh itself, run `GRAFTYARD_SERVE=18471 cargo test -p graftyard-api --test gh_serve --
--ignored`, and have the TLS capture server forward to that port.
