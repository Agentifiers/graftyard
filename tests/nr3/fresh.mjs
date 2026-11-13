// narrow-reads-3: no API read answers older than a write acknowledged before it began (`design/DESIGN.md`, "Follower
// read"). Learners now answer a narrow read from the newest answer at or above the read's floor, and a read that waits
// for a build is answered later at the learner's state then (`raft::Member::fresh_after`). On a live install under
// load, each round comments on one of REPO's issues through the edge (`addComment`, acknowledged), then at once reads
// the issue's comments back on REST (`GET /issues/{n}/comments`) and its count on GraphQL, both narrow reads served by
// the repository's learners and followers, and checks both include the new comment. Run beside a B0-29 load so reads wait for builds. Prints one JSON line per round
// and a summary, and exits 1 on any stale answer.
//   REPO=nr3-after ROUNDS=200 node tests/nr3/fresh.mjs
const { api, tokens, sleep } = await import("../am1/hosted/lib.mjs");

const ORG = "m3org";
const REPO = process.env.REPO;
const ROUNDS = Number(process.env.ROUNDS ?? 200);
if (!REPO) throw new Error("REPO is required");

const gql = (query, variables) => api("POST", "/api/graphql", tokens.admin, { query, variables });
const made = await gql("query($r: String!) { repository(owner: \"m3org\", name: $r) { id } }", { r: REPO });
const repoId = made.body?.data?.repository?.id;
if (!repoId) throw new Error(`no repository ${REPO}: ${JSON.stringify(made.body).slice(0, 200)}`);
const issue = (await gql("mutation($i: CreateIssueInput!) { createIssue(input: $i) { issue { id number } } }",
  { i: { repositoryId: repoId, title: "nr3 fresh 0" } })).body?.data?.createIssue?.issue;
if (!issue) throw new Error("could not create the probe issue");

// a1.mjs comments on the repository's first issue too, so the probe's issue may share it: each round reads the
// issue's comment count first, then checks the newest page of comments (where the new one lands) and that the count
// moved past the one read before the write.
const countOf = async () => (await gql("query($r: String!, $n: Int!) { repository(owner: \"m3org\", name: $r) { issue(number: $n) { comments { totalCount } } } }", { r: REPO, n: issue.number }));
let stale = 0, ok = 0, failed = 0;
for (let k = 1; k <= ROUNDS; k++) {
  const body = `nr3 fresh ${k} ${Date.now()}`;
  const before = (await countOf()).body?.data?.repository?.issue?.comments?.totalCount;
  const w = await gql("mutation($i: AddCommentInput!) { addComment(input: $i) { subject { id } } }", { i: { subjectId: issue.id, body } });
  if (before === undefined || w.status !== 200 || !w.body?.data?.addComment) { failed++; console.log(JSON.stringify({ k, write: w.status })); continue; }
  const page = Math.ceil((before + 1) / 100);
  const [rest, g] = await Promise.all([
    api("GET", `/api/v3/repos/${ORG}/${REPO}/issues/${issue.number}/comments?per_page=100&page=${page}`, tokens.admin),
    countOf(),
  ]);
  const restHas = Array.isArray(rest.body) && rest.body.some((c) => c.body === body);
  const count = g.body?.data?.repository?.issue?.comments?.totalCount;
  // A read that did not answer 200 carries no claim; one that answered must include the acknowledged comment. The REST
  // page is the one the comment lands on unless other comments pushed it to the next page meanwhile.
  const pushedOn = Array.isArray(rest.body) && rest.body.length === 100;
  const bad = (rest.status === 200 && !restHas && !pushedOn) || (g.status === 200 && count !== undefined && count <= before);
  if (bad) stale++; else if (rest.status === 200 && g.status === 200) ok++; else failed++;
  console.log(JSON.stringify({ k, before, rest: rest.status, restHas, pushedOn, graphql: g.status, count: count ?? null, ms: [rest.ms, g.ms], stale: bad }));
  await sleep(250);
}
console.log(JSON.stringify({ summary: true, rounds: ROUNDS, ok, stale, failed, issue: issue.number }));
process.exit(stale ? 1 : 0);
