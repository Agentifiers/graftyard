import { http, HttpResponse } from "msw";
import type { PersistedRequest, PullMergeState } from "@/api/graphql";
import persisted from "@/api/persisted.json";
import { gate } from "../http";
import { db } from "./core-db";
import { summary, sync } from "./pulls-db";

// POST /api/graphql, persisted documents only, as the edge serves it. openapi-msw is
// REST-only, so this uses msw's own http. The first PullMergeState read of each pull
// request answers UNKNOWN, as GHES does while it computes the test merge, so the merge
// box's polling runs.

const asked = new Set<string>();

const STATES = new Set([
  "BEHIND",
  "BLOCKED",
  "CLEAN",
  "DIRTY",
  "DRAFT",
  "HAS_HOOKS",
  "UNKNOWN",
  "UNSTABLE",
]);

type Pr = NonNullable<NonNullable<PullMergeState["repository"]>["pullRequest"]>;

const fail = (message: string) => HttpResponse.json({ data: null, errors: [{ message }] });

export const handlers = [
  http.post("/api/graphql", async ({ request }) => {
    const fault = await gate("graphql");
    if (fault) return fault;
    const body = (await request.json()) as PersistedRequest;
    const hash = body.extensions?.persistedQuery?.sha256Hash;
    const doc = Object.entries(persisted).find(([, d]) => d.sha256 === hash);
    if (!doc) return fail("PersistedQueryNotFound");
    if (doc[0] !== "PullMergeState") return fail(`Unknown operation ${doc[0]}`);
    const { owner, name, number } = body.variables as {
      owner: string;
      name: string;
      number: number;
    };
    const state = db.state(owner, name);
    const pr = state?.pulls.find((p) => p.number === number);
    if (!state || !pr) return HttpResponse.json({ data: { repository: null } });
    sync(state, pr);
    const key = `${owner}/${name}#${number}`;
    const first = !asked.has(key);
    asked.add(key);
    const computed = pr.state === "open" && !first && pr.mergeable !== null;
    const status = pr.mergeable_state.toUpperCase();
    const data: PullMergeState = {
      repository: {
        pullRequest: {
          mergeable: !computed ? "UNKNOWN" : pr.mergeable ? "MERGEABLE" : "CONFLICTING",
          mergeStateStatus: (computed && STATES.has(status)
            ? status
            : "UNKNOWN") as Pr["mergeStateStatus"],
          reviewDecision: summary(state, pr).review_decision,
          headRefOid: pr.head.sha,
        },
      },
    };
    return HttpResponse.json({ data });
  }),
];
