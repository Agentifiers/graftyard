// BF-34 on graft-m2b (`build/BUILD-PLAN.md`, M3-WP3), through the edge's GHES 3.22 routes, `runs` times; each run
// makes three fresh users (`m3r<tag><i>-in`, `-out`, `-no`) with a classic PAT each:
// - org invitations: m3-admin invites `-in` and `-no` to m3org (`Invite` on the org's `Authz` chain; GHES 3.22's
//   `POST /orgs/{org}/invitations` is not built here). `-in` accepts with `PATCH /user/memberships/orgs/m3org`
//   (200, `active`), `-no` declines with `DELETE` on the same path (204) and its membership is then 404, and a later
//   PATCH by `-no` finds no invitation (404).
// - collaborators: `PUT /repos/m3org/j4/collaborators/m3-member` (an org member) is refused 422; m3-admin invites
//   `-out` (204), `-out` lists it with `GET /user/repository_invitations?repo=m3org/j4` and accepts with PATCH (204),
//   and is then in `GET .../collaborators` with `push`, 204 on `GET .../collaborators/{u}`, `write` on
//   `.../permission`. `-in`, a member since its accept, is refused 422 too, and m3-member (Write, not Admin) is
//   refused an invitation (403). m3-admin invites `-no` and withdraws it with `DELETE /repos/m3org/j4/invitations/{id}`
//   (204), after which `-no`'s PATCH is 404 and `-no` is no collaborator.
// - teams: `POST /orgs/m3org/teams` (201), `PUT .../memberships/m3-member` (200), `PUT .../repos/m3org/j4` (204),
//   `GET .../teams/{slug}` (200).
// Every reply a GHES 3.22 operation documents is written to <replies>/<case>.rest.json for `validate.py`.
//   node tests/m3/rows/bf34.mjs <runs> <tag> <out.jsonl> <replies dir>
import { mkdirSync, writeFileSync } from "node:fs";
import { api, as, log, must, pat, row, scan, state, tokens, user } from "./lib.mjs";

const [runsArg, tag, out, replies] = process.argv.slice(2);
const runs = Number(runsArg ?? 5);
if (!/^[a-z]$/.test(tag ?? "")) throw new Error("tag: one letter a..z, so a rerun makes fresh users");
mkdirSync(replies, { recursive: true });
const ORG = "m3org", REPO = "m3org/j4";
const admin = tokens.admin, member = tokens.member;
const base = 7_150_000 + (tag.charCodeAt(0) - 97) * 1_000;

/** A REST call whose reply is kept as a validate.py case when `op` names the documented operation. */
async function call(name, op, method, path, token, body) {
  const r = await api(method, path, token, body);
  if (op) writeFileSync(`${replies}/${name}.rest.json`, scan(JSON.stringify({ method, path: op, status: r.status, body: r.body, location: null }, null, 1)));
  return r;
}

const c = { runs, ok: 0, fail: [] };
for (let i = 1; i <= runs; i++) {
  const n = `m3r${tag}${i}`;
  const ids = { in: base + i * 10 + 1, out: base + i * 10 + 2, no: base + i * 10 + 3 };
  const tok = {};
  for (const [k, a] of Object.entries(ids)) { await user(a, `${n}-${k}`); tok[k] = (await pat(a, `bf34 ${k}`)).text; }
  const k = (s) => `r${i}-${s}`;
  const r = {};
  // ---- the org invitations ----
  const inv = async (a) => (await must(`authz:${state.org}`, { cred: as(state.admin), op: { Invite: { invitee: { User: a }, role: "Member", teams: [] } } })).ok.out.Invitation;
  r.inviteIn = await inv(ids.in);
  r.inviteNo = await inv(ids.no);
  r.pendingIn = (await call(k("org-pending"), "/user/memberships/orgs/{org}", "GET", `/user/memberships/orgs/${ORG}`, tok.in)).body?.state;
  const acc = await call(k("org-accept"), "/user/memberships/orgs/{org}", "PATCH", `/user/memberships/orgs/${ORG}`, tok.in, { state: "active" });
  r.accept = [acc.status, acc.body?.state, acc.body?.role];
  r.declineStatus = (await call(k("org-decline"), null, "DELETE", `/user/memberships/orgs/${ORG}`, tok.no)).status;
  r.noAfterDecline = (await call(k("org-declined-get"), null, "GET", `/user/memberships/orgs/${ORG}`, tok.no)).status;
  r.acceptAfterDecline = (await call(k("org-accept-after"), null, "PATCH", `/user/memberships/orgs/${ORG}`, tok.no, { state: "active" })).status;
  r.inActive = (await call(k("org-active"), "/user/memberships/orgs/{org}", "GET", `/user/memberships/orgs/${ORG}`, tok.in)).body?.state;
  // ---- collaborators ----
  const mem = await call(k("collab-member"), "/repos/{owner}/{repo}/collaborators/{username}", "PUT", `/repos/${REPO}/collaborators/m3-member`, admin, { permission: "push" });
  r.memberWrite = mem.status;
  r.newMemberWrite = (await call(k("collab-new-member"), null, "PUT", `/repos/${REPO}/collaborators/${n}-in`, admin, { permission: "push" })).status;
  r.inviteOut = (await call(k("collab-invite"), "/repos/{owner}/{repo}/collaborators/{username}", "PUT", `/repos/${REPO}/collaborators/${n}-out`, admin, { permission: "push" })).status;
  r.byMember = (await call(k("collab-by-member"), null, "PUT", `/repos/${REPO}/collaborators/${n}-no`, member, { permission: "push" })).status;
  const pend = await call(k("repo-invitations"), "/repos/{owner}/{repo}/invitations", "GET", `/repos/${REPO}/invitations`, admin);
  const mine = await call(k("user-invitations"), "/user/repository_invitations", "GET", `/user/repository_invitations?repo=${REPO}`, tok.out);
  const outInv = mine.body?.[0];
  r.outListed = [Array.isArray(pend.body) && pend.body.some((x) => x.invitee?.login === `${n}-out`), mine.body?.length];
  r.outAccept = outInv ? (await call(k("user-accept"), "/user/repository_invitations/{invitation_id}", "PATCH", `/user/repository_invitations/${outInv.id}?repo=${REPO}`, tok.out)).status : "no invitation";
  const list = await call(k("collaborators"), "/repos/{owner}/{repo}/collaborators", "GET", `/repos/${REPO}/collaborators`, admin);
  const outRow = Array.isArray(list.body) ? list.body.find((x) => x.login === `${n}-out`) : null;
  r.outInList = outRow ? outRow.permissions?.push === true : false;
  r.memberInList = Array.isArray(list.body) && list.body.some((x) => x.login === "m3-member");
  r.outCheck = (await call(k("collaborator-check"), "/repos/{owner}/{repo}/collaborators/{username}", "GET", `/repos/${REPO}/collaborators/${n}-out`, admin)).status;
  r.outPerm = (await call(k("collaborator-permission"), "/repos/{owner}/{repo}/collaborators/{username}/permission", "GET", `/repos/${REPO}/collaborators/${n}-out/permission`, admin)).body?.permission;
  // The withdrawn invitation: `-no` invited, then cancelled by m3-admin; `-no` can no longer accept it.
  r.inviteNoRepo = (await api("PUT", `/repos/${REPO}/collaborators/${n}-no`, admin, { permission: "pull" })).status;
  const noInv = (await api("GET", `/user/repository_invitations?repo=${REPO}`, tok.no)).body?.[0];
  r.cancel = noInv ? (await call(k("invitation-delete"), "/repos/{owner}/{repo}/invitations/{invitation_id}", "DELETE", `/repos/${REPO}/invitations/${noInv.id}`, admin)).status : "no invitation";
  r.acceptCancelled = noInv ? (await api("PATCH", `/user/repository_invitations/${noInv.id}?repo=${REPO}`, tok.no)).status : null;
  r.noCheck = (await api("GET", `/repos/${REPO}/collaborators/${n}-no`, admin)).status;
  // ---- teams ----
  const team = await call(k("team-create"), "/orgs/{org}/teams", "POST", `/orgs/${ORG}/teams`, admin, { name: `${n} Team` });
  const slug = team.body?.slug;
  r.team = [team.status, slug];
  r.teamMember = (await call(k("team-membership"), "/orgs/{org}/teams/{team_slug}/memberships/{username}", "PUT", `/orgs/${ORG}/teams/${slug}/memberships/m3-member`, admin)).status;
  r.teamRepo = (await call(k("team-repo"), "/orgs/{org}/teams/{team_slug}/repos/{owner}/{repo}", "PUT", `/orgs/${ORG}/teams/${slug}/repos/${REPO}`, admin, { permission: "push" })).status;
  const tg = await call(k("team-get"), "/orgs/{org}/teams/{team_slug}", "GET", `/orgs/${ORG}/teams/${slug}`, admin);
  r.teamGet = [tg.status, tg.body?.members_count];
  r.teamsList = (await call(k("teams"), "/orgs/{org}/teams", "GET", `/orgs/${ORG}/teams`, member)).status;
  r.teamByMember = (await api("POST", `/orgs/${ORG}/teams`, member, { name: `${n} Nope` })).status;
  const pass = r.pendingIn === "pending" && r.accept.join() === "200,active,member" && r.declineStatus === 204
    && r.noAfterDecline === 404 && r.acceptAfterDecline === 404 && r.inActive === "active"
    && r.memberWrite === 422 && r.newMemberWrite === 422 && r.inviteOut === 204 && r.byMember === 403
    && r.outListed[0] === true && r.outListed[1] === 1 && r.outAccept === 204 && r.outInList && r.memberInList
    && r.outCheck === 204 && r.outPerm === "write" && r.inviteNoRepo === 204 && r.cancel === 204
    && r.acceptCancelled === 404 && r.noCheck === 404
    && r.team[0] === 201 && r.teamMember === 200 && r.teamRepo === 204 && r.teamGet.join() === "200,1"
    && r.teamsList === 200 && r.teamByMember === 403;
  if (pass) c.ok++; else c.fail.push(i);
  row(out, { i, pass, ...r, inviteIn: undefined, inviteNo: undefined });
  log(`BF-34 ${i}/${runs}: ${pass ? "pass" : "FAIL"} ${JSON.stringify(r)}`);
}
row(out, { summary: c });
log(`BF-34: ${JSON.stringify(c)}`);

