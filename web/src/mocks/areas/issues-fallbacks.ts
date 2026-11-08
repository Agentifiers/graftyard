import type { Schemas } from "@/api/client";
import { API } from "../world";
import { gate, http } from "../http";

// Minimal handlers for reads and writes another area owns (notifications belong to
// activity). The merge deletes this file once the owner's handlers land. The issue page
// uses the issue's id as its notification thread id.

const subscriptions = new Map<number, Schemas["thread-subscription"]>();

function subscription(threadId: number): Schemas["thread-subscription"] {
  return (
    subscriptions.get(threadId) ?? {
      subscribed: false,
      ignored: false,
      reason: null,
      created_at: null,
      url: `${API}/notifications/threads/${threadId}/subscription`,
      thread_url: `${API}/notifications/threads/${threadId}`,
    }
  );
}

export const handlers = [
  http.get("/notifications/threads/{thread_id}/subscription", async ({ params, response }) => {
    const fault = await gate("issues-subscription");
    if (fault) return response.untyped(fault);
    return response(200).json(subscription(Number(params.thread_id)));
  }),

  http.put(
    "/notifications/threads/{thread_id}/subscription",
    async ({ params, request, response }) => {
      const fault = await gate("issues-subscribe");
      if (fault) return response.untyped(fault);
      const threadId = Number(params.thread_id);
      const text = await request.text();
      const ignored = Boolean(text && JSON.parse(text)?.ignored);
      const next = {
        ...subscription(threadId),
        subscribed: !ignored,
        ignored,
        reason: ignored ? null : "manual",
        created_at: new Date().toISOString(),
      };
      subscriptions.set(threadId, next);
      return response(200).json(next);
    },
  ),

  http.delete("/notifications/threads/{thread_id}/subscription", async ({ params, response }) => {
    const fault = await gate("issues-subscribe");
    if (fault) return response.untyped(fault);
    subscriptions.delete(Number(params.thread_id));
    return response(204).empty();
  }),
];
