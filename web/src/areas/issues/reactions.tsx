import { useMutation, useQuery, useQueryClient, useSuspenseQuery } from "@tanstack/react-query";
import { HugeiconsIcon } from "@hugeicons/react";
import { SmilePlusIcon } from "@hugeicons/core-free-icons";
import { api, type Schemas } from "@/api/client";
import { userQuery } from "@/areas/core/queries";
import { send, toastFailure } from "./api";
import { commentReactionsQuery, issueReactionsQuery } from "./queries";
import { Region } from "@/components/region";
import { Button } from "@/components/ui/button";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { cn } from "cn";

type Reaction = Schemas["reaction"];
type Content = Reaction["content"];

// GitHub's eight reactions, in its picker order.
export const REACTIONS: { content: Content; emoji: string; label: string }[] = [
  { content: "+1", emoji: "👍", label: "Thumbs up" },
  { content: "-1", emoji: "👎", label: "Thumbs down" },
  { content: "laugh", emoji: "😄", label: "Laugh" },
  { content: "hooray", emoji: "🎉", label: "Hooray" },
  { content: "confused", emoji: "😕", label: "Confused" },
  { content: "heart", emoji: "❤️", label: "Heart" },
  { content: "rocket", emoji: "🚀", label: "Rocket" },
  { content: "eyes", emoji: "👀", label: "Eyes" },
];

export type ReactionSubject =
  | { kind: "issue"; owner: string; repo: string; number: number }
  | { kind: "comment"; owner: string; repo: string; id: number };

function queryFor(s: ReactionSubject) {
  return s.kind === "issue" ? issueReactionsQuery(s) : commentReactionsQuery(s);
}

// The reaction row under a post. Like the pull request's tab counts, a failing reactions
// read shows nothing rather than an error on every post. A negative comment id is an
// optimistic comment with nothing to react to.
export function Reactions(s: ReactionSubject) {
  if (s.kind === "comment" && s.id < 0) return null;
  return (
    <Region name="reactions" fallback={<div className="h-7" />}>
      <ReactionBar {...s} />
    </Region>
  );
}

function ReactionBar(s: ReactionSubject) {
  const queryClient = useQueryClient();
  const { data: viewer } = useSuspenseQuery(userQuery());
  const q = queryFor(s);
  const reactions = useQuery(q);
  const data = reactions.data ?? [];
  const mine = (c: Content) => data.find((r) => r.content === c && r.user?.login === viewer.login);

  const toggle = useMutation({
    mutationFn: async (content: Content) => {
      const existing = mine(content);
      const path = { owner: s.owner, repo: s.repo };
      if (existing) {
        await send(
          s.kind === "issue"
            ? api.DELETE("/repos/{owner}/{repo}/issues/{issue_number}/reactions/{reaction_id}", {
                params: { path: { ...path, issue_number: s.number, reaction_id: existing.id } },
              })
            : api.DELETE(
                "/repos/{owner}/{repo}/issues/comments/{comment_id}/reactions/{reaction_id}",
                {
                  params: { path: { ...path, comment_id: s.id, reaction_id: existing.id } },
                },
              ),
        );
        return;
      }
      await send(
        s.kind === "issue"
          ? api.POST("/repos/{owner}/{repo}/issues/{issue_number}/reactions", {
              params: { path: { ...path, issue_number: s.number } },
              body: { content },
            })
          : api.POST("/repos/{owner}/{repo}/issues/comments/{comment_id}/reactions", {
              params: { path: { ...path, comment_id: s.id } },
              body: { content },
            }),
      );
    },
    onMutate: async (content) => {
      await queryClient.cancelQueries({ queryKey: q.queryKey });
      const before = queryClient.getQueryData(q.queryKey);
      const existing = mine(content);
      queryClient.setQueryData(q.queryKey, (old = []) =>
        existing
          ? old.filter((r) => r.id !== existing.id)
          : [
              ...old,
              {
                id: -Date.now(),
                node_id: "",
                user: viewer,
                content,
                created_at: new Date().toISOString(),
              },
            ],
      );
      return { before };
    },
    onError: (e, _content, ctx) => {
      queryClient.setQueryData(q.queryKey, ctx?.before);
      toastFailure("Reaction", e);
    },
    onSettled: () => queryClient.invalidateQueries({ queryKey: q.queryKey }),
  });

  if (reactions.isError && !reactions.data) return null;
  if (!reactions.data) return <div className="h-7" />;

  const counts = REACTIONS.map((r) => ({
    ...r,
    users: data.filter((x) => x.content === r.content).map((x) => x.user?.login ?? "ghost"),
  })).filter((r) => r.users.length > 0);

  return (
    <div className="flex flex-wrap items-center gap-1" data-testid="reactions">
      <Popover>
        <PopoverTrigger
          render={
            <Button variant="outline" size="icon-xs" aria-label="Add reaction">
              <HugeiconsIcon icon={SmilePlusIcon} strokeWidth={2} />
            </Button>
          }
        />
        <PopoverContent align="start" className="w-auto flex-row gap-0.5 rounded-sm p-1">
          {REACTIONS.map((r) => (
            <button
              key={r.content}
              type="button"
              aria-label={r.label}
              aria-pressed={Boolean(mine(r.content))}
              title={r.label}
              className="hover:bg-muted aria-pressed:bg-primary/15 flex size-8 items-center justify-center rounded-sm text-base"
              onClick={() => toggle.mutate(r.content)}
            >
              {r.emoji}
            </button>
          ))}
        </PopoverContent>
      </Popover>
      {counts.map((r) => {
        const on = r.users.includes(viewer.login);
        return (
          <button
            key={r.content}
            type="button"
            aria-pressed={on}
            data-reaction={r.content}
            title={`${r.users.join(", ")} reacted with ${r.label.toLowerCase()}`}
            onClick={() => toggle.mutate(r.content)}
            className={cn(
              "flex h-6 items-center gap-1 rounded-sm border px-1.5 text-xs tabular-nums",
              on
                ? "border-primary/50 bg-primary/10 text-foreground"
                : "bg-background hover:bg-muted",
            )}
          >
            <span aria-hidden>{r.emoji}</span>
            <span>{r.users.length}</span>
          </button>
        );
      })}
    </div>
  );
}
