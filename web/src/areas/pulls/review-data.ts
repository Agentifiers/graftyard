import { useQuery, useSuspenseQuery } from "@tanstack/react-query";
import { userQuery } from "@/areas/core/queries";
import {
  pendingCommentsQuery,
  reviewCommentsQuery,
  reviewsQuery,
  threadsQuery,
  type Pull,
} from "./queries";
import { threadsOf, type ReviewComment } from "./threads";

// Reviews, review comments, and thread state as one model: submitted threads from
// GET .../comments, plus the viewer's pending review and its comments, which GHES shows
// only to its author and lists only through GET .../reviews/{id}/comments.
export function useReviewModel(pull: Pull) {
  const { data: viewer } = useSuspenseQuery(userQuery());
  const { data: reviews } = useSuspenseQuery(reviewsQuery(pull));
  const { data: comments } = useSuspenseQuery(reviewCommentsQuery(pull));
  const { data: states } = useSuspenseQuery(threadsQuery(pull));
  const pending = reviews.find((r) => r.state === "PENDING" && r.user?.login === viewer.login);
  const { data: pendingRaw = [] } = useQuery({
    ...pendingCommentsQuery({ ...pull, reviewId: pending?.id ?? 0 }),
    enabled: Boolean(pending),
  });
  const pendingComments: ReviewComment[] = pending
    ? pendingRaw.map((c) => ({ ...c, position: c.position ?? undefined }))
    : [];
  const threads = threadsOf(
    [...comments, ...pendingComments],
    states,
    new Set(pending ? [pending.id] : []),
  );
  return { viewer, reviews, pending, pendingComments, threads };
}

export type ReviewModel = ReturnType<typeof useReviewModel>;
