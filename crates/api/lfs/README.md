# What git-lfs and gh release send, and the oracles their replies are checked against

`requests.json` holds every request, in order, that two pinned clients sent to a local capture server for the
`lfs`, `rest:releases`, `uploads-release-assets`, and `codeload-archives` surfaces (`build/BUILD-PLAN.md`, M2-WP4).
`tests/blobs.rs` replays it through the handlers and checks each reply's status against what the client got.
The three GraphQL documents gh sent are in `../gh/` as `RepositoryReleaseByTag-1.json`, `RepositoryReleaseList-1.json`,
and `Release_fields-1.json`.

## The clients

| Client | Version | sha256 of the binary |
| --- | --- | --- |
| git | 2.56.0 | `2912a2a32f408c9b8122e476251b32f4b35f4c3f0c15961686ae87ca456d384c` |
| git-lfs | 3.8.0 (go 1.27.0) | `9ab4f28af5b23b6428d30a10bccdad69c515759ea5fabd0a0e0f1bc0dbf29fc2` |
| gh | 2.102.1 | `30bfdd4abead53dab4452ec507dd3641480dd168ad3b39edd86e2338191a77eb` |

## How they were captured

1. As in `../gh/README.md`, a local TLS server listened on `127.0.0.1:443` as `ghes.graftyard.test`, which both
   clients classify as GHES. It answered GraphQL from the pinned GHES 3.22 SDL, served git through `git http-backend`,
   and answered LFS batch, transfer, verify, and locks, and the release REST and upload routes, from in-memory state.
2. The clients ran with an empty `HOME`, `GH_HOST=ghes.graftyard.test`, a fake `GH_ENTERPRISE_TOKEN`, and a credential
   helper that answered a fake password. No GitHub credential was used, and nothing went to github.com.
3. These commands ran once each, in this order, and all exited 0:

   ```
   git lfs install; git clone; git lfs track '*.bin'; git push (two LFS objects, 3,000,000 and 1,000 bytes)
   git lfs locks; git lfs lock a.bin; git lfs locks; git lfs unlock --force a.bin; git lfs push --all origin
   git clone (second clone); git lfs fetch --all; git lfs pull
   gh release create v1 --generate-notes; gh release upload v1 ./app.tgz; gh release upload v1 ./notes.txt --clobber
   gh release list; gh release view v1; gh release download v1; gh release download v1 -p 'app.*' --clobber
   gh release download v1 --archive=tar.gz; gh release download v1 --archive=zip
   gh release create v2 --notes x --target main app.tgz; gh release upload v1 ./app.tgz --clobber
   gh release edit v1 --title T; gh release delete-asset v1 notes.txt -y; gh release view v1 --json ...
   gh release delete v2 -y --cleanup-tag
   ```

That made 87 requests. The 22 git smart HTTP requests (`info/refs`, `git-upload-pack`, `git-receive-pack`) are M1's
surface and are left out. `requests.json` holds the other 65:

- 19 LFS requests from git-lfs/3.8.0: 4 `objects/batch`, 2 `PUT` and 2 `GET` transfers, 2 `objects/verify`, 4
  `locks/verify`, 3 `GET locks`, 1 `POST locks`, and 1 `locks/{id}/unlock`. Two of them (one `locks/verify`, one
  `objects/batch`) are the first, unauthenticated try that git-lfs repeats after a 401.
- 14 GraphQL posts from gh, holding the 3 documents above.
- 32 REST, upload, and codeload requests from gh.

Request bodies that are not JSON (the transfer and asset bytes) are kept as their size and sha256. The `Authorization`
value is replaced by `present`.

## The oracles

- REST: `ghes-3.22.json`, pinned in `../gh/README.md`.
- GraphQL: `schema.docs-enterprise.graphql`, pinned in `../gh/README.md`.
- LFS: git-lfs 3.8.0's own JSON schemas, in `schemas/`, from `github.com/git-lfs/git-lfs/v3@v3.8.0` (`tq/schemas` and
  `locking/schemas`, fetched through `proxy.golang.org`). Their sha256 values:

  | File | sha256 |
  | --- | --- |
  | `http-batch-request-schema.json` | `07484745b9b7a0f138292b094e8eff83748f834163d112c6bdfc492fe092ea75` |
  | `http-batch-response-schema.json` | `e066cd359156cc9541c8cc68f95be5b37ca1ab2f88f54dac274a50c37e8e6072` |
  | `http-lock-create-request-schema.json` | `249422d5a8fdcbfe6249a51b22d6393341140abd7d5ccb2fb52ea8f39e47e0c5` |
  | `http-lock-create-response-schema.json` | `4ec5f027216cd082fec5af7563067a4729e123dd0c92ceb82993096c249dbbba` |
  | `http-lock-delete-request-schema.json` | `407ec2d62185611d54de73dfe2d9c9b9c0e55f97181fb87734767eb768698e63` |
  | `http-lock-list-response-schema.json` | `6273cefbe4a095f35be7ec17af21e7fa4a7c1cfbc2abefb6ea1384a36ae87231` |
  | `http-lock-verify-response-schema.json` | `dbb59e81a6a0f4b6f9a62dd9d3ed379dec155442da6e3cf5aade0eea09f63759` |

`../gh/validate.py` checks every reply `tests/blobs.rs` writes; see `../gh/README.md` for the commands.

## What the handlers leave out

- Transfers other than `basic`. git-lfs offered `lfs-standalone-file`, `basic`, and `ssh`; the batch answers `basic`.
- The SSH half of `lfs` (`git-lfs-authenticate` and `git-lfs-transfer`), which is M6-WP2.
- Objects over `BLOB_MAX` (100 MiB): the batch answers a per-object 422. Multipart uploads are not built.
- Lock `cursor` and `limit` paging: every lock comes back in one page.
- `generate_release_notes` writes the tag as the name and an empty body; GitHub's notes need the git cache's commit
  walk. `POST /releases/generate-notes` itself was not sent by gh and is not served.
- `GET /releases` (REST list), `GET /releases/latest`, `GET /releases/{id}/assets`, asset `PATCH`, and
  `/{o}/{r}/releases/download/{tag}/{name}` (the browser URL): gh did not send them.
