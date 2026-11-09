import { useMutation, useMutationState, useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import { api, unwrap } from "@/api/client";
import { ui } from "./ui-api";
import type { Thread } from "./queries";

// Everything the inbox can do to threads, as data. `apply` is the optimistic view of an
// action, `send` performs it, and `inverse` is what "Undo" sends. While an action is in
// flight its variables overlay every list (useMutationState), so a poll that lands
// mid-flight never resurrects a thread, and a failure rolls back by simply ending.

export type Action =
  | { kind: "read" | "unread" | "done" | "inbox" | "unsubscribe" | "resubscribe"; ids: string[] }
  | { kind: "save" | "unsave"; ids: string[] }
  | { kind: "read-all"; before: string; repo?: { owner: string; repo: string } };

export type View = "inbox" | "saved" | "done";

// A thread as a list shows it: its inbox state plus whether it is saved.
export type Row = Thread & { saved: boolean };

const has = (a: Action, id: string) => "ids" in a && a.ids.includes(id);

// The rows a view shows after an action, given the rows the server last sent.
export function apply(rows: Row[], a: Action, view: View): Row[] {
  switch (a.kind) {
    case "done":
    case "unsubscribe":
      return view === "inbox" || view === "saved" ? rows.filter((r) => !has(a, r.id)) : rows;
    case "inbox":
    case "resubscribe":
      return view === "done" ? rows.filter((r) => !has(a, r.id)) : rows;
    case "read":
    case "unread":
      return rows.map((r) => (has(a, r.id) ? { ...r, unread: a.kind === "unread" } : r));
    case "save":
      return rows.map((r) => (has(a, r.id) ? { ...r, saved: true } : r));
    case "unsave":
      return view === "saved"
        ? rows.filter((r) => !has(a, r.id))
        : rows.map((r) => (has(a, r.id) ? { ...r, saved: false } : r));
    case "read-all":
      return rows.map((r) =>
        r.updated_at <= a.before &&
        (!a.repo || r.repository.full_name === `${a.repo.owner}/${a.repo.repo}`)
          ? { ...r, unread: false }
          : r,
      );
  }
}

// What Undo sends. Mark-all-read has no inverse on GHES, so it gets no Undo.
export function inverse(a: Action): Action | undefined {
  switch (a.kind) {
    case "read":
      return { kind: "unread", ids: a.ids };
    case "unread":
      return { kind: "read", ids: a.ids };
    case "done":
      return { kind: "inbox", ids: a.ids };
    case "inbox":
      return { kind: "done", ids: a.ids };
    case "unsubscribe":
      return { kind: "resubscribe", ids: a.ids };
    case "resubscribe":
      return { kind: "unsubscribe", ids: a.ids };
    case "save":
      return { kind: "unsave", ids: a.ids };
    case "unsave":
      return { kind: "save", ids: a.ids };
    case "read-all":
      return undefined;
  }
}

const each = (ids: string[], call: (thread_id: string) => Promise<unknown>) =>
  Promise.all(ids.map(call));

const path = (thread_id: string) => ({ params: { path: { thread_id } } });
// The REST thread id is an integer in the description, a string in the thread body.
const restPath = (thread_id: string) => ({ params: { path: { thread_id: Number(thread_id) } } });

async function send(a: Action) {
  switch (a.kind) {
    case "read":
      return each(a.ids, (id) =>
        unwrap(api.PATCH("/notifications/threads/{thread_id}", restPath(id))),
      );
    case "unread":
      return each(a.ids, (id) =>
        unwrap(ui.PUT("/notifications/threads/{thread_id}/unread", path(id))),
      );
    case "done":
      return each(a.ids, (id) =>
        unwrap(api.DELETE("/notifications/threads/{thread_id}", restPath(id))),
      );
    case "inbox":
      return each(a.ids, (id) => unwrap(ui.DELETE("/notifications/done/{thread_id}", path(id))));
    // GHES's inbox unsubscribes and marks the thread done in one click.
    case "unsubscribe":
      return each(a.ids, async (id) => {
        await unwrap(api.DELETE("/notifications/threads/{thread_id}/subscription", restPath(id)));
        await unwrap(api.DELETE("/notifications/threads/{thread_id}", restPath(id)));
      });
    case "resubscribe":
      return each(a.ids, async (id) => {
        await unwrap(
          api.PUT("/notifications/threads/{thread_id}/subscription", {
            ...restPath(id),
            body: { ignored: false },
          }),
        );
        await unwrap(ui.DELETE("/notifications/done/{thread_id}", path(id)));
      });
    case "save":
      return each(a.ids, (id) => unwrap(ui.PUT("/notifications/saved/{thread_id}", path(id))));
    case "unsave":
      return each(a.ids, (id) => unwrap(ui.DELETE("/notifications/saved/{thread_id}", path(id))));
    case "read-all":
      return a.repo
        ? unwrap(
            api.PUT("/repos/{owner}/{repo}/notifications", {
              params: { path: a.repo },
              body: { last_read_at: a.before },
            }),
          )
        : unwrap(api.PUT("/notifications", { body: { last_read_at: a.before, read: true } }));
  }
}

function describe(a: Action) {
  const n = "ids" in a ? a.ids.length : 0;
  const what = n === 1 ? "1 notification" : `${n} notifications`;
  switch (a.kind) {
    case "read":
      return `Marked ${what} as read`;
    case "unread":
      return `Marked ${what} as unread`;
    case "done":
      return `Marked ${what} as done`;
    case "inbox":
      return `Moved ${what} to the inbox`;
    case "unsubscribe":
      return `Unsubscribed from ${what}`;
    case "resubscribe":
      return `Subscribed to ${what} again`;
    case "save":
      return `Saved ${what}`;
    case "unsave":
      return `Removed ${what} from saved`;
    case "read-all":
      return a.repo
        ? `Marked everything in ${a.repo.owner}/${a.repo.repo} as read`
        : "Marked everything as read";
  }
}

const PRESENT: Record<string, string> = {
  Marked: "mark",
  Moved: "move",
  Unsubscribed: "unsubscribe",
  Subscribed: "subscribe",
  Saved: "save",
  Removed: "remove",
};
const failed = (a: Action) => describe(a).replace(/^\w+/, (w) => `Could not ${PRESENT[w]}`);

const KEY = ["notifications", "action"];

export function useNotificationAction() {
  const queryClient = useQueryClient();
  const mutation = useMutation({
    mutationKey: KEY,
    mutationFn: send,
    // Settling waits for the refetch, so the overlay holds until the lists show the result.
    onSettled: () =>
      Promise.all([
        queryClient.invalidateQueries({ queryKey: ["notifications"] }),
        queryClient.invalidateQueries({ queryKey: ["_t", "ui", "notifications"] }),
      ]),
  });
  // Every action reports itself. An undo reports too, but offers no undo of its own.
  // Opening a thread marks it read quietly; it still toasts if that fails.
  const run = (a: Action, { undoable = true, quiet = false } = {}) =>
    mutation.mutate(a, {
      onSuccess: () => {
        if (quiet) return;
        const back = undoable ? inverse(a) : undefined;
        toast.success(describe(a), {
          action: back && { label: "Undo", onClick: () => run(back, { undoable: false }) },
        });
      },
      onError: (e) => toast.error(failed(a), { description: e.message }),
    });
  return run;
}

// The actions in flight right now, oldest first, for the overlay.
export function usePendingActions(): Action[] {
  return useMutationState({
    filters: { mutationKey: KEY, status: "pending" },
    select: (m) => m.state.variables as Action,
  });
}
