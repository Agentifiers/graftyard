// The agents loop's own strings, every product string from the code:
//   the refusal         crates/git/src/agent.rs, AgentRefusal::of(AgentRule::Merge)
//   the gate            crates/git/src/merge.rs, gate_holds: "refused: graftyard/agent-gate is not satisfied for
//                       <short>: <have> of <need> approvals from human writers"; the card shows its count, have of need
//   the agent's branch  a Graftyard-run task's refs live under refs/heads/graftyard/<uuid>/ (crates/git/src/task.rs
//                       RUN_PREFIX); the session id 7f3c stands for the uuid, short as the loop shows it
// The push's reply is git's own report of a new branch (its column padding cut to two spaces so the row fits the
// window). The reviewer's name is illustrative.

export const SESSION = '7f3c';
export const BRANCH = `graftyard/${SESSION}/fix-session`;

export const TERMINAL = {
  title: `agent · session ${SESSION}`,
  prompt: '$',
  push: `git push origin HEAD:${BRANCH}`,
  pushReply: ['To git.example', ` * [new branch]  HEAD -> ${BRANCH}`],
  /** The reply's row and the characters set in the accent: the push landing. */
  landed: { row: 1, text: '[new branch]' },
  merge: 'gh pr merge 412',
  /** AgentRule::Merge's refusal, set in the refusal colour. */
  refusal: 'refused: agents may not merge pull requests',
};

export const PR = {
  title: 'Pull request #412',
  branch: BRANCH,
  check: 'graftyard/agent-gate',
  /** gate_holds' count: `have` of `need` approvals from human writers (need is 1 with a recorded requester). */
  need: 1,
  approvals: 'approvals from human writers',
};

export const REVIEW = { title: 'Reviewers', who: 'mara', state: 'approved' };
export const QUEUE = { title: 'Merge queue', pr: '#412', state: 'merged' };
