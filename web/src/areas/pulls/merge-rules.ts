// What stands between a pull request and a merge. Pure, so the mock decides
// `mergeable_state` with the same rules the merge box draws its checklist from.

export type Requirements = {
  required_checks: string[];
  required_approvals: number;
  // Require the head to be up to date with the base before merging.
  strict: boolean;
};

type Run = { name: string; status: string; conclusion: string | null };
type Review = { user: { login: string } | null; state: string; submitted_at?: string };

export type CheckState = "success" | "failure" | "pending" | "none";
export type ReviewDecision = "APPROVED" | "CHANGES_REQUESTED" | "REVIEW_REQUIRED";

const FAILED = new Set(["failure", "timed_out", "action_required", "cancelled", "startup_failure"]);

export function runState(run: Run): CheckState {
  if (run.status !== "completed") return "pending";
  return run.conclusion && FAILED.has(run.conclusion) ? "failure" : "success";
}

export function checkRollup(runs: Run[]) {
  const states = runs.map(runState);
  const count = (s: CheckState) => states.filter((x) => x === s).length;
  const failed = count("failure");
  const pending = count("pending");
  const state: CheckState =
    runs.length === 0 ? "none" : failed ? "failure" : pending ? "pending" : "success";
  return { state, passed: count("success"), failed, pending, total: runs.length };
}

// Each reviewer's standing verdict: the latest approval, change request, or dismissal.
// A later plain comment does not withdraw an approval.
export function verdicts(reviews: Review[]): Map<string, string> {
  const out = new Map<string, string>();
  const ordered = [...reviews].sort((a, b) =>
    (a.submitted_at ?? "").localeCompare(b.submitted_at ?? ""),
  );
  for (const r of ordered) {
    if (!r.user || r.state === "PENDING") continue;
    if (r.state === "COMMENTED" && out.has(r.user.login)) continue;
    out.set(r.user.login, r.state);
  }
  return out;
}

// With no required approvals, any review without a change request satisfies the rule.
export function reviewDecision(reviews: Review[], required: number): ReviewDecision {
  const v = [...verdicts(reviews).values()];
  if (v.includes("CHANGES_REQUESTED")) return "CHANGES_REQUESTED";
  return v.filter((s) => s === "APPROVED").length >= required ? "APPROVED" : "REVIEW_REQUIRED";
}

export type Requirement = {
  key: "draft" | "conflicts" | "reviews" | "checks" | "behind";
  state: "ok" | "pending" | "failing";
  title: string;
  detail: string;
};

export type MergeInput = {
  draft: boolean;
  runs: Run[];
  reviews: Review[];
  requirements: Requirements;
  behind_by: number;
  conflicts: boolean;
};

const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? "" : "s"}`;

export function evaluate(input: MergeInput): {
  mergeable_state: "clean" | "unstable" | "blocked" | "behind" | "dirty" | "draft";
  requirements: Requirement[];
} {
  const { requirements: req } = input;
  const list: Requirement[] = [];

  if (input.draft)
    list.push({
      key: "draft",
      state: "failing",
      title: "This pull request is still a work in progress",
      detail: "Draft pull requests cannot be merged. Mark it ready for review first.",
    });

  if (input.conflicts)
    list.push({
      key: "conflicts",
      state: "failing",
      title: "This branch has conflicts that must be resolved",
      detail: "Resolve the conflicts on the head branch, then push.",
    });

  const decision = reviewDecision(input.reviews, req.required_approvals);
  const approvals = [...verdicts(input.reviews).values()].filter((s) => s === "APPROVED").length;
  list.push(
    decision === "APPROVED"
      ? {
          key: "reviews",
          state: "ok",
          title: approvals ? "Changes approved" : "No review required",
          detail: approvals
            ? `${plural(approvals, "approving review")}.`
            : "The base branch does not require approving reviews.",
        }
      : decision === "CHANGES_REQUESTED"
        ? {
            key: "reviews",
            state: "failing",
            title: "Changes requested",
            detail: "A reviewer asked for changes. Address them and ask for another review.",
          }
        : {
            key: "reviews",
            state: "failing",
            title: "Review required",
            detail: `At least ${plural(req.required_approvals, "approving review")} by a reviewer with write access is required.`,
          },
  );

  const byName = new Map(input.runs.map((r) => [r.name, runState(r)]));
  const required = req.required_checks.map((name) => byName.get(name) ?? "pending");
  const all = checkRollup(input.runs);
  const reqFailed = required.filter((s) => s === "failure").length;
  const reqPending = required.filter((s) => s === "pending").length;
  list.push(
    reqFailed
      ? {
          key: "checks",
          state: "failing",
          title: `${plural(reqFailed, "required check")} failed`,
          detail: `${all.passed} of ${all.total} checks passed.`,
        }
      : reqPending
        ? {
            key: "checks",
            state: "pending",
            title: `${plural(reqPending, "required check")} pending`,
            detail: "Waiting for status to be reported.",
          }
        : {
            key: "checks",
            state: all.state === "success" || all.state === "none" ? "ok" : "pending",
            title:
              all.state === "failure"
                ? "Some checks were not successful"
                : all.state === "pending"
                  ? "Some checks have not completed"
                  : "All checks have passed",
            detail: `${all.passed} of ${all.total} checks passed${req.required_checks.length ? ", every required check among them" : ""}.`,
          },
  );

  // A conflicted branch cannot be updated in place, so only the conflict row asks for work.
  if (input.behind_by > 0 && !input.conflicts)
    list.push({
      key: "behind",
      state: req.strict ? "failing" : "pending",
      title: "This branch is out of date with the base branch",
      detail: `${plural(input.behind_by, "commit")} behind. ${req.strict ? "Update the branch before merging." : "Updating is optional."}`,
    });

  const mergeable_state = input.draft
    ? "draft"
    : input.conflicts
      ? "dirty"
      : decision !== "APPROVED" || reqFailed || reqPending
        ? "blocked"
        : input.behind_by > 0 && req.strict
          ? "behind"
          : all.state === "failure" || all.state === "pending"
            ? "unstable"
            : "clean";
  return { mergeable_state, requirements: list };
}

export const MERGEABLE = new Set(["clean", "unstable", "has_hooks"]);

// The commit keywords that close an issue when the pull request merges.
export function linkedIssues(body: string | null | undefined): number[] {
  const out = new Set<number>();
  for (const m of (body ?? "").matchAll(/\b(?:close[sd]?|fix(?:e[sd])?|resolve[sd]?)\s+#(\d+)\b/gi))
    out.add(Number(m[1]));
  return [...out];
}
