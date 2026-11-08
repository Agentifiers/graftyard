import { useState } from "react";
import { Link } from "@tanstack/react-router";
import { useMutation, useQueryClient, useSuspenseQuery } from "@tanstack/react-query";
import { useForm } from "@tanstack/react-form";
import { toast } from "sonner";
import { HugeiconsIcon } from "@hugeicons/react";
import { ArrowDown01Icon, StarIcon, ViewIcon, ViewOffIcon } from "@hugeicons/core-free-icons";
import { api, ApiError, unwrap, type Schemas } from "@/api/client";
import { repoQuery } from "@/areas/core/queries";
import { ButtonGroup } from "@/components/ui/button-group";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuGroup,
  DropdownMenuLabel,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import {
  starredQuery,
  watchersQuery,
  watchQuery,
  type WatchMode,
  type WatchState,
} from "./queries";
import { ui, WATCH_EVENTS, type WatchEvent } from "./ui-api";

type Repo = { owner: string; repo: string };
type FullRepo = Schemas["full-repository"];

// ---- Star ----

// Toggles at once: the button and the count move before the PUT or DELETE answers, and
// snap back with a toast if it fails.
export function StarButton(r: Repo) {
  const queryClient = useQueryClient();
  const { data: starred } = useSuspenseQuery(starredQuery(r));
  const { data: repo } = useSuspenseQuery(repoQuery(r));
  const star = useMutation({
    mutationFn: async (next: boolean) => {
      const path = { params: { path: r } };
      await unwrap(
        next
          ? api.PUT("/user/starred/{owner}/{repo}", path)
          : api.DELETE("/user/starred/{owner}/{repo}", path),
      );
    },
    onMutate: async (next) => {
      const keys = [starredQuery(r).queryKey, repoQuery(r).queryKey];
      await Promise.all(keys.map((queryKey) => queryClient.cancelQueries({ queryKey })));
      const before = { starred, repo: queryClient.getQueryData<FullRepo>(repoQuery(r).queryKey) };
      queryClient.setQueryData(starredQuery(r).queryKey, next);
      queryClient.setQueryData<FullRepo>(
        repoQuery(r).queryKey,
        (old) => old && { ...old, stargazers_count: old.stargazers_count + (next ? 1 : -1) },
      );
      return before;
    },
    onError: (e, next, before) => {
      queryClient.setQueryData(starredQuery(r).queryKey, before?.starred);
      queryClient.setQueryData(repoQuery(r).queryKey, before?.repo);
      toast.error(
        next ? `Could not star ${r.owner}/${r.repo}` : `Could not unstar ${r.owner}/${r.repo}`,
        {
          description: e.message,
        },
      );
    },
    onSettled: () =>
      Promise.all([
        ...[starredQuery(r), repoQuery(r)].map(({ queryKey }) =>
          queryClient.invalidateQueries({ queryKey }),
        ),
        queryClient.invalidateQueries({ queryKey: ["repos", r.owner, r.repo, "stargazers"] }),
        queryClient.invalidateQueries({ queryKey: ["user", "starred"], exact: true }),
      ]),
  });
  return (
    <ButtonGroup>
      <Button
        variant="outline"
        size="sm"
        className="rounded-sm"
        aria-pressed={starred}
        onClick={() => star.mutate(!starred)}
        data-testid="star-button"
      >
        <HugeiconsIcon
          icon={StarIcon}
          strokeWidth={2}
          data-icon="inline-start"
          className={starred ? "fill-warning text-warning" : undefined}
        />
        {starred ? "Starred" : "Star"}
      </Button>
      <Button
        variant="outline"
        size="sm"
        className="rounded-sm tabular-nums"
        render={<Link to="/$owner/$repo/stargazers" params={r} />}
        nativeButton={false}
        aria-label={`${repo.stargazers_count} stargazers`}
        data-testid="star-count"
      >
        {repo.stargazers_count}
      </Button>
    </ButtonGroup>
  );
}

// ---- Watch ----

const MODES: Record<
  Exclude<WatchMode, "custom">,
  { label: string; hint: string; body?: { subscribed: boolean; ignored: boolean } }
> = {
  participating: {
    label: "Participating and @mentions",
    hint: "Only threads you take part in or are mentioned in.",
  },
  all: {
    label: "All activity",
    hint: "Every issue, pull request, and release.",
    body: { subscribed: true, ignored: false },
  },
  ignore: {
    label: "Ignore",
    hint: "Never notify, even when @mentioned.",
    body: { subscribed: false, ignored: true },
  },
};

export const EVENT_LABELS: Record<WatchEvent, string> = {
  issues: "Issues",
  pulls: "Pull requests",
  releases: "Releases",
  discussions: "Discussions",
  security_alerts: "Security alerts",
};

export function WatchButton(r: Repo) {
  const queryClient = useQueryClient();
  const { data: watch } = useSuspenseQuery(watchQuery(r));
  const { data: watchers } = useSuspenseQuery(watchersQuery(r));
  const [customOpen, setCustomOpen] = useState(false);

  const set = useMutation({
    mutationFn: async (mode: Exclude<WatchMode, "custom">) => {
      const path = { params: { path: r } };
      // Leaving custom clears its event list first.
      if (watch.mode === "custom")
        await unwrap(ui.DELETE("/repos/{owner}/{repo}/subscription/events", path));
      const body = MODES[mode].body;
      await unwrap(
        body
          ? api.PUT("/repos/{owner}/{repo}/subscription", { ...path, body })
          : api.DELETE("/repos/{owner}/{repo}/subscription", path),
      );
    },
    onMutate: async (mode) => {
      await queryClient.cancelQueries({ queryKey: watchQuery(r).queryKey });
      const before = queryClient.getQueryData<WatchState>(watchQuery(r).queryKey);
      queryClient.setQueryData<WatchState>(watchQuery(r).queryKey, { mode, events: [] });
      return before;
    },
    onError: (e, mode, before) => {
      queryClient.setQueryData(watchQuery(r).queryKey, before);
      toast.error(`Could not change watching to "${MODES[mode].label}"`, {
        description: e.message,
      });
    },
    onSuccess: (_, mode) => toast.success(`Watching ${r.owner}/${r.repo}: ${MODES[mode].label}`),
    onSettled: () =>
      Promise.all(
        [watchQuery(r), watchersQuery(r)].map(({ queryKey }) =>
          queryClient.invalidateQueries({ queryKey }),
        ),
      ),
  });

  const watching = watch.mode === "all" || watch.mode === "custom";
  const label = watch.mode === "ignore" ? "Ignoring" : watching ? "Unwatch" : "Watch";
  return (
    <>
      <ButtonGroup>
        <DropdownMenu>
          <DropdownMenuTrigger
            render={
              <Button
                variant="outline"
                size="sm"
                className="rounded-sm"
                data-testid="watch-button"
                data-mode={watch.mode}
              >
                <HugeiconsIcon
                  icon={watch.mode === "ignore" ? ViewOffIcon : ViewIcon}
                  strokeWidth={2}
                  data-icon="inline-start"
                />
                {label}
                <HugeiconsIcon icon={ArrowDown01Icon} strokeWidth={2} data-icon="inline-end" />
              </Button>
            }
          />
          <DropdownMenuContent align="end" className="w-72 rounded-sm">
            <DropdownMenuGroup>
              <DropdownMenuLabel>
                Notifications for {r.owner}/{r.repo}
              </DropdownMenuLabel>
              <DropdownMenuRadioGroup
                value={watch.mode}
                onValueChange={(v) => {
                  if (v === "custom") setCustomOpen(true);
                  else if (v !== watch.mode) set.mutate(v as Exclude<WatchMode, "custom">);
                }}
              >
                {(Object.keys(MODES) as Exclude<WatchMode, "custom">[]).map((m) => (
                  <DropdownMenuRadioItem
                    key={m}
                    value={m}
                    className="items-start rounded-sm"
                    data-testid={`watch-${m}`}
                  >
                    <span className="flex flex-col">
                      <span className="font-medium">{MODES[m].label}</span>
                      <span className="text-muted-foreground text-xs">{MODES[m].hint}</span>
                    </span>
                  </DropdownMenuRadioItem>
                ))}
                <DropdownMenuRadioItem
                  value="custom"
                  className="items-start rounded-sm"
                  data-testid="watch-custom"
                  closeOnClick
                >
                  <span className="flex flex-col">
                    <span className="font-medium">Custom</span>
                    <span className="text-muted-foreground text-xs">
                      {watch.mode === "custom"
                        ? watch.events.map((e) => EVENT_LABELS[e]).join(", ")
                        : "Choose the events, as well as participating and @mentions."}
                    </span>
                  </span>
                </DropdownMenuRadioItem>
              </DropdownMenuRadioGroup>
            </DropdownMenuGroup>
          </DropdownMenuContent>
        </DropdownMenu>
        <Button
          variant="outline"
          size="sm"
          className="rounded-sm tabular-nums"
          render={<Link to="/$owner/$repo/watchers" params={r} />}
          nativeButton={false}
          aria-label={`${watchers.length} watchers`}
          data-testid="watch-count"
        >
          {watchers.length}
        </Button>
      </ButtonGroup>
      <CustomWatchDialog repo={r} open={customOpen} onOpenChange={setCustomOpen} current={watch} />
    </>
  );
}

// Custom watching: a TanStack Form over the event checkboxes. The client requires one
// event; the server's 422 (such as discussions on a repository without them) lands on
// the field.
function CustomWatchDialog({
  repo: r,
  open,
  onOpenChange,
  current,
}: {
  repo: Repo;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  current: WatchState;
}) {
  const queryClient = useQueryClient();
  const form = useForm({
    defaultValues: {
      events: current.mode === "custom" ? current.events : (["issues", "pulls"] as WatchEvent[]),
    },
    onSubmit: async ({ value, formApi }) => {
      const { data, error, response } = await ui.PUT("/repos/{owner}/{repo}/subscription/events", {
        params: { path: r },
        body: { events: value.events },
      });
      if (response.status === 422 && error) {
        const messages = (error.errors ?? []).map((e) => e.message ?? `${e.field} is ${e.code}`);
        formApi.setFieldMeta("events", (m) => ({
          ...m,
          errorMap: { ...m.errorMap, onServer: messages.join(" ") },
        }));
        return;
      }
      if (!response.ok || !data) {
        toast.error("Could not save custom watching", {
          description: new ApiError(response.status, error).message,
        });
        return;
      }
      queryClient.setQueryData<WatchState>(watchQuery(r).queryKey, {
        mode: "custom",
        events: data.events ?? [],
      });
      void queryClient.invalidateQueries({ queryKey: watchQuery(r).queryKey });
      void queryClient.invalidateQueries({ queryKey: watchersQuery(r).queryKey });
      toast.success(`Watching ${r.owner}/${r.repo}: custom`);
      onOpenChange(false);
    },
  });
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="rounded-sm sm:max-w-sm" data-testid="custom-watch">
        <DialogHeader>
          <DialogTitle>Custom notifications</DialogTitle>
          <DialogDescription>
            Get participating and @mentions for {r.owner}/{r.repo}, plus every event you check.
          </DialogDescription>
        </DialogHeader>
        <form
          onSubmit={(e) => {
            e.preventDefault();
            void form.handleSubmit();
          }}
          className="flex flex-col gap-3"
        >
          <form.Field
            name="events"
            validators={{
              onChange: ({ value }) => (value.length ? undefined : "Choose at least one event."),
            }}
          >
            {(field) => (
              <fieldset className="flex flex-col gap-2">
                <legend className="sr-only">Events</legend>
                {WATCH_EVENTS.map((ev) => (
                  <label key={ev} className="flex items-center gap-2 text-sm">
                    <Checkbox
                      checked={field.state.value.includes(ev)}
                      onCheckedChange={(on) =>
                        field.handleChange(
                          on
                            ? [...field.state.value, ev]
                            : field.state.value.filter((x) => x !== ev),
                        )
                      }
                      data-testid={`event-${ev}`}
                    />
                    {EVENT_LABELS[ev]}
                  </label>
                ))}
                {field.state.meta.errors.length > 0 && (
                  <p
                    className="text-destructive text-xs"
                    role="alert"
                    data-testid="custom-watch-error"
                  >
                    {field.state.meta.errors.join(" ")}
                  </p>
                )}
              </fieldset>
            )}
          </form.Field>
          <DialogFooter>
            <Button
              type="button"
              variant="outline"
              size="sm"
              className="rounded-sm"
              onClick={() => onOpenChange(false)}
            >
              Cancel
            </Button>
            <form.Subscribe selector={(s) => [s.isSubmitting, s.canSubmit] as const}>
              {([submitting, canSubmit]) => (
                <Button
                  type="submit"
                  size="sm"
                  className="rounded-sm"
                  disabled={submitting || !canSubmit}
                >
                  {submitting ? "Saving…" : "Apply"}
                </Button>
              )}
            </form.Subscribe>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
