// Unit tests for M0-WP10's pure `decide`, `classify`, globs, trailers, and SESSION_REFS listing.
//   cd tests/m0 && node --test agent/tests/
import { test } from "node:test";
import assert from "node:assert/strict";
import { decide, apply, type Ctx } from "../../src/agent/decide.ts";
import { classify } from "../../src/agent/classify.ts";
import { globMatch, globOverlap } from "../../src/agent/glob.ts";
import { parseTrailers, provenanceOf, sessionFromUrl } from "../../src/agent/provenance.ts";
import { lsRefsVisible, advertisedV01 } from "../../src/agent/smart.ts";
import { changedOf, protectedOf } from "../../src/agent/validate.ts";
import { APP, BASE, CREDS, FORK, HOST, INST, T1, T2, T3, T4, USER, X1, X2, authz, baseFold, forkFold } from "../../src/agent/fixture.ts";
import type { Cmd, Credential, Fold, Provenance, RefUpdate } from "../../src/agent/model.ts";

const AT = 1_800_000_000_000;
const A = "a".repeat(40), B = "b".repeat(40), C = "c".repeat(40);

function bed(): { base: Fold; fork: Fold } {
  const base = baseFold(AT);
  base.refs["refs/heads/main"] = A;
  base.refs[`refs/heads/graftyard/${T1}/work`] = B;
  base.refs["refs/heads/claude/feat"] = B;
  base.prs["1"] = { number: 1, headRef: `refs/heads/graftyard/${T1}/work`, headOid: B, base: "refs/heads/main", draft: false, open: true, merged: false,
    meta: { app: APP.agent, session: T1, requester: USER.alice, repo: BASE }, gate: null, author: `a${APP.agent}i${INST.agent}`, pushers: [] };
  base.nextPr = 2;
  const fork = forkFold();
  fork.refs["refs/heads/main"] = A;
  return { base, fork };
}

function ctx(label: string | Credential, opt: { fold?: Fold; base?: Fold; header?: string; prov?: Provenance[]; at?: number } = {}): Ctx {
  const b = bed();
  const fold = opt.fold ?? b.base;
  const cred = typeof label === "string" ? CREDS[label] : label;
  const hasTask = cred.t === "Installation" && cred.task != null;
  const base = fold.forkOf && hasTask ? opt.base ?? b.base : null;
  const az = authz();
  const p = classify(cred, opt.header ?? null, az, fold, base, opt.prov ?? []);
  return { fold, deps: { authz: az, base }, actor: p, at: opt.at ?? AT + 1000 };
}

const push = (updates: RefUpdate[], extra: Partial<Extract<Cmd, { t: "Push" }>> = {}): Cmd =>
  ({ t: "Push", via: "git", updates, atomic: false, changed: [], protected: [], provenance: [], facts: updates.map((u) => ({ name: u.name, ff: true, missing: false })), ...extra });
const rule = (d: ReturnType<typeof decide>) => d.ok ? (d.refs?.find((r) => !r.ok) as any)?.refusal.rule ?? "ok" : d.refusal.rule;

const AGENTS = ["grun-ws", "vendor"] as const;
const OTHERS = ["flagged", "mcp-bob", "bob"] as const;

test("classify: the five kinds of the rows", () => {
  const k = (l: string, h?: string) => { const c = ctx(l, { header: h }); return `${c.actor.kind.t}/${c.actor.admit}`; };
  assert.equal(k("grun-ws"), "Agent/Agent");
  assert.equal(k("grun-agent"), "Agent/Agent");
  assert.equal(k("vendor"), "Agent/Agent");
  assert.equal(k("flagged"), "Bot/Agent");
  assert.equal(k("mcp-bob"), "User/Agent");
  assert.equal(k("bob"), "User/Human");
  assert.equal(k("bob", T2), "User/Agent");
  // requester comes from the task row, not from the caller.
  const c = ctx("grun-ws");
  assert.deepEqual(c.actor.kind.t === "Agent" && c.actor.kind.agent, { app: APP.workspace, task: T1, requester: USER.alice });
  // A vendor's session header resolves its bare name to its own External task.
  const v = ctx("vendor", { header: `${APP.claude}.fix-1` });
  assert.equal(v.actor.kind.t === "Agent" && v.actor.kind.agent.task, X1);
});

test("classify: a trailer classes an App push but never a user-token push", () => {
  const prov: Provenance[] = [{ commit: A, sources: [{ t: "Claimed", key: "ClaudeSession", value: "x" }] }];
  const co: Provenance[] = [{ commit: A, sources: [{ t: "Claimed", key: "CoAuthoredBy", value: "Claude <c@x>" }] }];
  const insights: Credential = { t: "Installation", app: APP.insights, installation: INST.insights, task: null };
  assert.equal(ctx(insights, { prov }).actor.admit, "Agent");
  assert.equal(ctx(insights, { prov: co }).actor.admit, "Bot");
  assert.equal(ctx("bob", { prov }).actor.admit, "Human");
  assert.equal(ctx("bob", { prov: co }).actor.admit, "Human");
});

const refusalMatrix: [string, Cmd, string][] = [
  ["approve", { t: "Review", pr: 1, event: "APPROVE" }, "Approve"],
  ["merge", { t: "Merge", pr: 1, sha: null, mergeOid: null }, "Merge"],
  ["ready", { t: "ReadyForReview", pr: 1 }, "ReadyForReview"],
  ["ruleset edit", { t: "RulesetEdit", id: 7 }, "Bypass"],
  ["branch protection edit", { t: "BranchProtectionEdit", branch: "main" }, "Bypass"],
  ["repository dispatch", { t: "RepositoryDispatch" }, "Dispatch"],
  ["workflow dispatch", { t: "WorkflowDispatch", workflow: "ci.yml", ref: "main" }, "Dispatch"],
  ["re-run", { t: "WorkflowRerun", run: 1 }, "Rerun"],
  ["release", { t: "ReleaseCreate", tag: "v1" }, "Release"],
  ["label", { t: "LabelWrite", issue: 1, labels: ["bug"] }, "Label"],
  ["graftyard/ status", { t: "StatusCreate", sha: A, context: "graftyard/agent-gate" }, "GraftyardStatus"],
  ["graftyard/ check run", { t: "CheckRunCreate", sha: A, name: "graftyard/overlap" }, "GraftyardStatus"],
  ["rename", { t: "BranchRename", from: "main", to: "trunk" }, "Rename"],
  ["hook", { t: "HookEdit" }, "Hook"],
  ["deploy key", { t: "DeployKeyEdit" }, "DeployKey"],
];

for (const [name, cmd, want] of refusalMatrix) {
  test(`refusal: ${name} refused for both agent kinds, allowed for the other three`, () => {
    for (const a of AGENTS) assert.equal(rule(decide(cmd, ctx(a))), want, a);
    for (const o of OTHERS) {
      const d = decide(cmd, ctx(o));
      if (cmd.t === "Merge") assert.notEqual(rule(d), "Merge", o); // humans hit the gate instead
      else if (cmd.t === "Review" && o === "mcp-bob") assert.equal(rule(d), "McpScope");
      else assert.equal(rule(d), "ok", `${o}: ${JSON.stringify(d)}`);
    }
  });
}

test("labels inside the allowlist are allowed for an agent", () => {
  assert.equal(rule(decide({ t: "LabelWrite", issue: 1, labels: ["agent:ok"] }, ctx("grun-agent"))), "ok");
  assert.equal(rule(decide({ t: "StatusCreate", sha: A, context: "ci/test" }, ctx("vendor"))), "ok");
});

test("push: prefix, live task, protected, host-owned", () => {
  const pre = `refs/heads/graftyard/${T1}/`;
  assert.equal(rule(decide(push([{ name: `${pre}x`, old: null, new: C }]), ctx("grun-ws"))), "ok");
  assert.equal(rule(decide(push([{ name: "refs/heads/x", old: null, new: C }]), ctx("grun-ws"))), "OutsidePrefix");
  assert.equal(rule(decide(push([{ name: `refs/heads/graftyard/${T2}/x`, old: null, new: C }]), ctx("grun-ws"))), "OutsidePrefix");
  assert.equal(rule(decide(push([{ name: `${pre}x`, old: null, new: C }]), ctx("grun-ws-notask"))), "NoLiveTask");
  assert.equal(rule(decide(push([{ name: `refs/heads/graftyard/${T4}/x`, old: null, new: C }]), ctx("grun-ws-done"))), "NoLiveTask");
  assert.equal(rule(decide(push([{ name: "refs/heads/claude/x", old: null, new: C }]), ctx("vendor"))), "ok");
  assert.equal(rule(decide(push([{ name: "refs/heads/cursor/x", old: null, new: C }]), ctx("vendor"))), "OutsidePrefix");
  assert.equal(rule(decide(push([{ name: "refs/tags/v1", old: null, new: C }]), ctx("vendor"))), "Release");
  const prot = { protected: [{ refname: `${pre}x`, paths: [".github/workflows/ci.yml"] }] };
  const d = decide(push([{ name: `${pre}x`, old: null, new: C }], prot), ctx("grun-ws"));
  assert.equal(rule(d), "Protected");
  assert.match((d as any).refusal.text, /\.github\/workflows\/ci\.yml/);
  // Humans and non-confined Apps may change protected paths (rulesets and CODEOWNERS govern them).
  assert.equal(rule(decide(push([{ name: "refs/heads/bob/x", old: null, new: C }], { protected: [{ refname: "refs/heads/bob/x", paths: ["CODEOWNERS"] }] }), ctx("bob"))), "ok");
  // A Graftyard-run task idle past TASK_IDLE reads as timed out.
  assert.equal(rule(decide(push([{ name: `${pre}x`, old: null, new: C }]), ctx("grun-ws", { at: AT + 86_400_001 }))), "NoLiveTask");
});

test("push: ruleset bypass is refused to an agent even when listed", () => {
  const name = `refs/heads/graftyard/${T1}/work`;
  const cmd = push([{ name, old: B, new: C }], { facts: [{ name, ff: false, missing: false }] });
  assert.equal(rule(decide(cmd, ctx("grun-ws"))), "Bypass");
  const cv = push([{ name: "refs/heads/claude/feat", old: B, new: C }], { facts: [{ name: "refs/heads/claude/feat", ff: false, missing: false }] });
  assert.equal(rule(decide(cv, ctx("vendor"))), "Bypass");
  // A listed human-less App that is not an agent uses the bypass (cursor is listed on ruleset 7).
  const cm = push([{ name: "refs/heads/main", old: A, new: C }], { facts: [{ name: "refs/heads/main", ff: false, missing: false }] });
  assert.equal(rule(decide(cm, ctx("flagged"))), "ok");
  assert.equal(rule(decide(cm, ctx("bob"))), "NotAllowed");
});

test("AgentsExclusive: agents refused with the lease named, the other three never", () => {
  const ch = (ref: string) => ({ changed: [{ refname: ref, paths: ["pkg/auth/session.go"], truncated: false }] });
  for (const [l, ref] of [["grun-ws", `refs/heads/graftyard/${T1}/l`], ["vendor", "refs/heads/claude/l"]] as const) {
    const d = decide(push([{ name: ref, old: null, new: C }], ch(ref)), ctx(l));
    assert.equal(rule(d), "Leased", l);
    assert.match((d as any).refs[0].refusal.text, /pkg\/auth\/\*\* is leased Exclusive by graftyard-agent \(task 22222222/);
  }
  for (const [l, ref] of [["flagged", "refs/heads/cursor/l"], ["mcp-bob", "refs/heads/m/l"], ["bob", "refs/heads/b/l"]] as const) {
    assert.equal(rule(decide(push([{ name: ref, old: null, new: C }], ch(ref)), ctx(l, { header: l === "bob" ? T2 : undefined }))), "ok", l);
  }
  // The holder's own task is not refused by its own lease.
  const own = `refs/heads/graftyard/${T2}/l`;
  assert.equal(rule(decide(push([{ name: own, old: null, new: C }], ch(own)), ctx("grun-ws-t2"))), "ok");
  // Truncated directories overlap any glob under them.
  const tr = { changed: [{ refname: `refs/heads/graftyard/${T1}/l`, paths: ["pkg/"], truncated: true }] };
  assert.equal(rule(decide(push([{ name: `refs/heads/graftyard/${T1}/l`, old: null, new: C }], tr), ctx("grun-ws"))), "Leased");
  // Advisory refuses nothing.
  const c = ctx("grun-ws"); c.fold.policy.leases = "Advisory";
  assert.equal(rule(decide(push([{ name: `refs/heads/graftyard/${T1}/l`, old: null, new: C }], ch(`refs/heads/graftyard/${T1}/l`)), c)), "ok");
  // Past EXCLUSIVE_MAX the lease reads as Shared; after `expires` it is gone; a cancelled holder task releases it.
  const late = ctx("grun-ws", { at: AT + 4 * 3_600_000 + 1 });
  late.fold.leases[0].expires = AT + 10 * 3_600_000;
  assert.equal(rule(decide(push([{ name: `refs/heads/graftyard/${T1}/l`, old: null, new: C }], ch(`refs/heads/graftyard/${T1}/l`)), late)), "ok");
  const cx = ctx("grun-ws"); cx.fold.tasks[T2].state = "Cancelled";
  assert.equal(rule(decide(push([{ name: `refs/heads/graftyard/${T1}/l`, old: null, new: C }], ch(`refs/heads/graftyard/${T1}/l`)), cx)), "ok");
});

test("fork mode: the fork group reads the task's state and the leases from the base", () => {
  const b = bed();
  const ref = `refs/heads/graftyard/${T3}/w`;
  assert.equal(rule(decide(push([{ name: ref, old: null, new: C }]), ctx("grun-ws-t3", { fold: b.fork, base: b.base }))), "ok");
  apply(b.base, [{ t: "TaskUpdated", session: T3, state: "Cancelled" }], AT);
  assert.equal(rule(decide(push([{ name: ref, old: null, new: C }]), ctx("grun-ws-t3", { fold: b.fork, base: b.base }))), "NoLiveTask");
  const lref = `refs/heads/graftyard/${T1}/w`;
  const ch = { changed: [{ refname: lref, paths: ["pkg/auth/a.go"], truncated: false }] };
  assert.equal(rule(decide(push([{ name: lref, old: null, new: C }], ch), ctx("grun-ws", { fold: b.fork, base: b.base }))), "Leased");
  // An uninstalled App's tasks read as cancelled.
  const c = ctx("grun-ws"); c.deps.authz.installations[INST.agent].active = false;
  assert.equal(rule(decide(push([{ name: `refs/heads/graftyard/${T1}/x`, old: null, new: C }]), c)), "NoLiveTask");
});

test("refs/graftyard/: only graftyard-workspace appends its own task's session log", () => {
  const s1 = `refs/graftyard/sessions/${T1}`;
  assert.equal(rule(decide(push([{ name: s1, old: null, new: C }]), ctx("grun-ws"))), "ok");
  for (const l of ["alice", "bob", "vendor", "flagged", "mcp-bob", "grun-agent"])
    assert.equal(rule(decide(push([{ name: s1, old: null, new: C }]), ctx(l))), "HostOwned", l);
  assert.equal(rule(decide(push([{ name: `refs/graftyard/sessions/${T2}`, old: null, new: C }]), ctx("grun-ws"))), "HostOwned");
  assert.equal(rule(decide(push([{ name: "refs/graftyard/other", old: null, new: C }]), ctx("grun-ws"))), "HostOwned");
  const c = ctx("grun-ws"); c.fold.refs[s1] = B;
  assert.equal(rule(decide(push([{ name: s1, old: B, new: null }]), c)), "HostOwned");
  assert.equal(rule(decide(push([{ name: s1, old: B, new: C }], { facts: [{ name: s1, ff: false, missing: false }] }), c)), "HostOwned");
  assert.equal(rule(decide(push([{ name: s1, old: B, new: C }]), c)), "ok");
});

test("AgentGate: head-bound, from a writer who is not the requester and did not push", () => {
  const b = bed();
  const c = (l: string) => ctx(l, { fold: b.base });
  assert.equal(rule(decide({ t: "Merge", pr: 1, sha: null, mergeOid: null }, c("carol"))), "GateUnsatisfied");
  // The requester's approval writes no gate; an MCP approval writes no gate.
  let d = decide({ t: "Review", pr: 1, event: "APPROVE" }, c("alice"));
  assert.ok(d.ok && !d.events.some((e) => e.t === "AgentGate"));
  d = decide({ t: "Review", pr: 1, event: "APPROVE" }, c("mcp-bob-am"));
  assert.ok(d.ok && !d.events.some((e) => e.t === "AgentGate"));
  d = decide({ t: "Review", pr: 1, event: "APPROVE" }, c("bob"));
  assert.ok(d.ok && d.events.some((e) => e.t === "AgentGate"));
  apply(b.base, (d as any).events, AT);
  // An agent push after approval moves the head; the gate no longer holds.
  apply(b.base, [{ t: "PrSync", pr: 1, head: C, pusher: `a${APP.workspace}i${INST.workspace}` }], AT);
  assert.equal(rule(decide({ t: "Merge", pr: 1, sha: null, mergeOid: null }, c("carol"))), "GateUnsatisfied");
  apply(b.base, (decide({ t: "Review", pr: 1, event: "APPROVE" }, c("bob")) as any).events, AT);
  assert.equal(rule(decide({ t: "Merge", pr: 1, sha: null, mergeOid: null }, c("carol"))), "ok");
  // A writer who pushed to the head cannot satisfy it.
  apply(b.base, [{ t: "PrSync", pr: 1, head: A, pusher: `u${USER.dave}` }], AT);
  d = decide({ t: "Review", pr: 1, event: "APPROVE" }, c("dave"));
  assert.ok(d.ok && !d.events.some((e) => e.t === "AgentGate"));
  // MCP merge needs the WriteApproveMerge scope.
  assert.equal(rule(decide({ t: "Merge", pr: 1, sha: null, mergeOid: null }, c("mcp-bob"))), "McpScope");
});

test("provenance: Session only for the matching task; forged trailers stay Claimed", () => {
  const url = (s: string, host = HOST, repo = BASE) => `https://${host}/${repo}/_graftyard/sessions/${s}`;
  const P = (v: string): Provenance[] => [{ commit: A, sources: [{ t: "Claimed", key: "GraftyardSession", value: v }] }];
  const kind = (l: string, v: string, fold?: Fold, base?: Fold) => {
    const c = ctx(l, { fold, base });
    const ref = c.fold.forkOf ? `refs/heads/graftyard/${T3}/p` : l === "vendor" ? "refs/heads/claude/p" : l.startsWith("grun") ? `refs/heads/graftyard/${T1}/p` : "refs/heads/p";
    const d = decide(push([{ name: ref, old: null, new: C }], { provenance: P(v) }), c);
    assert.ok(d.ok, JSON.stringify(d));
    return (d.events.find((e) => e.t === "Provenance") as any).list[0].sources[0].t;
  };
  assert.equal(kind("grun-ws", url(T1)), "Session");
  assert.equal(kind("grun-ws", url(T2)), "Claimed");
  assert.equal(kind("grun-ws", url(T1, "evil.example")), "Claimed");
  assert.equal(kind("grun-ws", url(T1).replace("https:", "http:")), "Claimed");
  assert.equal(kind("grun-ws", url(T1, HOST, "acme/other")), "Claimed");
  assert.equal(kind("grun-ws", url(T1) + "?x=1"), "Claimed");
  assert.equal(kind("bob", url(T1)), "Claimed");
  assert.equal(kind("flagged", url(T1)), "Claimed");
  assert.equal(kind("vendor", url(T1)), "Claimed");
  assert.equal(kind("vendor", url(X1)), "Session");
  const b = bed();
  assert.equal(kind("grun-ws-t3", url(T3), b.fork, b.base), "Session"); // fork mode names the base repository
  assert.equal(kind("grun-ws-t3", url(T3, HOST, FORK), b.fork, b.base), "Claimed");
  void X2;
});

test("trailer parser never throws and follows git's block rule", () => {
  assert.deepEqual(parseTrailers("subj\n\nbody\n\nGraftyard-Session: x\nCo-authored-by: A <a@b>\n").map((t) => t.key), ["GraftyardSession", "CoAuthoredBy"]);
  assert.deepEqual(parseTrailers("Graftyard-Session: x\n"), []);
  assert.deepEqual(parseTrailers("s\n\nGraftyard-Session: a\n  b\n")[0].value, "a b");
  assert.deepEqual(parseTrailers("s\r\n\r\nclaude-session: z\r\n")[0], { key: "ClaudeSession", value: "z" });
  assert.equal(parseTrailers("s\n\nGraftyard-Session: " + "x".repeat(5000))[0].value.length, 1024);
  assert.equal(parseTrailers("s\n\nGraftyard-Session: a\u202eb\u200b").at(0)?.value, "ab");
  const junk = new Uint8Array(4096).map((_, i) => (i * 131) & 255);
  const r = provenanceOf([{ oid: A, raw: junk }], [{ path: "zz", mode: "100644", type: "blob", oid: B, bytes: null }, { path: `${A.slice(0, 2)}/${A.slice(2)}`, mode: "100644", type: "blob", oid: C, bytes: null }]);
  assert.equal(r.parseErrors, 1);
  assert.deepEqual(r.list.find((x) => x.commit === A)?.sources.at(-1), { t: "AiNote", note: C });
  assert.equal(sessionFromUrl("not a url", HOST, BASE), null);
  assert.equal(sessionFromUrl(`https://user:pw@${HOST}/${BASE}/_graftyard/sessions/${T1}`, HOST, BASE), null);
});

test("globs and changed paths", () => {
  assert.ok(globMatch("pkg/auth/**", "pkg/auth/a/b.go"));
  assert.ok(!globMatch("pkg/auth/**", "pkg/authz/a.go"));
  assert.ok(globMatch("**/AGENTS.md", "AGENTS.md") && globMatch("**/AGENTS.md", "a/b/AGENTS.md"));
  assert.ok(globMatch(".github/workflows/**", ".github/workflows/ci.yml"));
  assert.ok(!globMatch("CODEOWNERS", "src/CODEOWNERS"));
  assert.ok(globMatch("refs/heads/claude/**", "refs/heads/claude/x/y"));
  assert.ok(globOverlap("pkg/*/x", "pkg/auth/**") && !globOverlap("pkg/a/**", "pkg/b/**") && globOverlap("pkg/", "pkg/auth/**"));
  const many = Array.from({ length: 1500 }, (_, i) => `d${i % 30}/f${i}.txt`);
  const c = changedOf("r", many);
  assert.ok(c.truncated && c.paths.length === 30 && c.paths[0].endsWith("/"));
  assert.deepEqual(protectedOf("r", ["a/b/CLAUDE.md", "src/x", "out/y"], ["out/**"])?.paths, ["a/b/CLAUDE.md", "out/y"]);
});

test("SESSION_REFS: never in v0/v1; v2 only to a ref-prefix that starts with it", () => {
  const s = `refs/graftyard/sessions/${T1}`;
  assert.deepEqual(advertisedV01([{ name: s, oid: A }, { name: "refs/heads/main", oid: A }]).map((r) => r.name), ["refs/heads/main"]);
  assert.equal(lsRefsVisible(s, []), false);
  assert.equal(lsRefsVisible(s, ["refs/"]), false);
  assert.equal(lsRefsVisible(s, ["refs/graftyard/"]), false);
  assert.equal(lsRefsVisible(s, ["refs/graftyard/sessions"]), false);
  assert.equal(lsRefsVisible(s, ["refs/graftyard/sessions/"]), true);
  assert.equal(lsRefsVisible(s, [`refs/graftyard/sessions/${T1.slice(0, 4)}`]), true);
  assert.equal(lsRefsVisible(s, ["refs/graftyard/sessions/9"]), false);
  assert.equal(lsRefsVisible("refs/graftyard/other", ["refs/graftyard/sessions/"]), false);
  assert.equal(lsRefsVisible("refs/heads/main", ["refs/graftyard/sessions/"]), false);
  assert.equal(lsRefsVisible("refs/heads/main", []), true);
});

test("A23: a PR with no recorded requester needs one more writer approval", () => {
  const b = bed();
  // A confined App's PR opened without X-Graftyard-Agent-Session: AgentMeta.requester is empty.
  b.base.prs["2"] = { number: 2, headRef: "refs/heads/claude/feat", headOid: B, base: "refs/heads/main", draft: false, open: true, merged: false,
    meta: { app: APP.claude, session: null, requester: null, repo: BASE }, gate: null, author: `a${APP.claude}i${INST.claude}`, pushers: [] };
  const c = (l: string) => ctx(l, { fold: b.base });
  const merge: Cmd = { t: "Merge", pr: 2, sha: null, mergeOid: null };
  // alice, who prompted the vendor, approves: the gate row is written, but one approval is not enough.
  let d = decide({ t: "Review", pr: 2, event: "APPROVE" }, c("alice"));
  assert.ok(d.ok && d.events.some((e) => e.t === "AgentGate"));
  apply(b.base, (d as any).events, AT);
  d = decide(merge, c("carol"));
  assert.equal(rule(d), "GateUnsatisfied");
  assert.match((d as any).refusal.text, /1 of 2 approvals/);
  // The same writer approving again does not count twice.
  apply(b.base, (decide({ t: "Review", pr: 2, event: "APPROVE" }, c("alice")) as any).events, AT);
  assert.equal(rule(decide(merge, c("carol"))), "GateUnsatisfied");
  // A second writer satisfies it.
  apply(b.base, (decide({ t: "Review", pr: 2, event: "APPROVE" }, c("bob")) as any).events, AT);
  assert.equal(rule(decide(merge, c("carol"))), "ok");
  // With a requester recorded, one approval from a non-requester is enough; extra_human_approvals adds to both.
  b.base.prs["2"].meta!.requester = USER.alice; b.base.prs["2"].approvals = []; b.base.prs["2"].gate = null;
  apply(b.base, (decide({ t: "Review", pr: 2, event: "APPROVE" }, c("bob")) as any).events, AT);
  assert.equal(rule(decide(merge, c("carol"))), "ok");
  b.base.policy.extraHumanApprovals = 1;
  assert.equal(rule(decide(merge, c("carol"))), "GateUnsatisfied");
});

test("A24: an Agent update of refs/notes/ai is refused as outside its prefix; others may write it", () => {
  for (const l of ["grun-ws", "vendor"]) {
    const d = decide(push([{ name: "refs/notes/ai", old: null, new: C }]), ctx(l, { header: l === "vendor" ? X1 : undefined }));
    assert.equal(rule(d), "OutsidePrefix", l);
  }
  for (const l of ["flagged", "mcp-bob", "bob"]) assert.equal(rule(decide(push([{ name: "refs/notes/ai", old: null, new: C }]), ctx(l))), "ok", l);
});

void T3;
