import { useEffect, useMemo, useReducer, useRef, useState } from "react";
import { Link, useNavigate } from "@tanstack/react-router";
import { useMutation, useSuspenseQuery } from "@tanstack/react-query";
import { useForm } from "@tanstack/react-form";
import { toast } from "sonner";
import {
  createTree,
  expandAllFeature,
  syncDataLoaderFeature,
  type TreeInstance,
} from "@headless-tree/core";
import { HugeiconsIcon } from "@hugeicons/react";
import { LockIcon, UserGroupIcon } from "@hugeicons/core-free-icons";
import { api, type Schemas } from "@/api/client";
import { Badge } from "@/components/reui/badge";
import { Tree, TreeItem, TreeItemLabel } from "@/components/reui/tree";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { NativeSelect, NativeSelectOption } from "@/components/ui/native-select";
import { RadioGroup, RadioGroupItem } from "@/components/ui/radio-group";
import { Textarea } from "@/components/ui/textarea";
import { send } from "./api";
import { applyServerErrors, EmptyRow, FieldMessage, FormFailure, refetch } from "./kit";
import { teamQuery, teamSettingsQuery, teamsQuery } from "./queries";

type Team = Schemas["team"];
type TeamFull = Schemas["team-full"];

// ---- The nesting tree ----

const ROOT = "__root__";

function index(teams: Team[]) {
  const byId = new Map(teams.map((t) => [String(t.id), t]));
  const children = new Map<string, string[]>([[ROOT, []]]);
  for (const t of teams) children.set(String(t.id), []);
  for (const t of teams) {
    const parent = t.parent && byId.has(String(t.parent.id)) ? String(t.parent.id) : ROOT;
    children.get(parent)!.push(String(t.id));
  }
  for (const list of children.values()) {
    list.sort((a, b) => byId.get(a)!.name.localeCompare(byId.get(b)!.name));
  }
  return { byId, children };
}

// The teams as ReUI's Tree over headless-tree: each team under its parent, every node
// expanded, a filter that keeps a match's ancestors so its place in the tree stays clear.
export function TeamTree({ org, filter }: { org: string; filter: string }) {
  const { data: teams } = useSuspenseQuery(teamsQuery(org));
  const q = filter.trim().toLowerCase();
  const visible = useMemo(() => {
    if (!q) return teams;
    const byId = new Map(teams.map((t) => [t.id, t]));
    const keep = new Set<number>();
    for (const t of teams) {
      if (!`${t.name} ${t.slug} ${t.description ?? ""}`.toLowerCase().includes(q)) continue;
      for (let at: Team | undefined = t; at; at = at.parent ? byId.get(at.parent.id) : undefined) {
        keep.add(at.id);
      }
    }
    return teams.filter((t) => keep.has(t.id));
  }, [teams, q]);
  const { byId, children } = useMemo(() => index(visible), [visible]);
  const [, rerender] = useReducer((n: number) => n + 1, 0);
  const treeRef = useRef<TreeInstance<Team | null> | null>(null);
  if (!treeRef.current) {
    treeRef.current = createTree<Team | null>({
      rootItemId: ROOT,
      getItemName: (item) => item.getItemData()?.name ?? "",
      isItemFolder: (item) => (children.get(item.getId())?.length ?? 0) > 0,
      dataLoader: {
        getItem: (id) => byId.get(id) ?? null,
        getChildren: (id) => children.get(id) ?? [],
      },
      initialState: { expandedItems: [...children.keys()] },
      setState: () => rerender(),
      features: [syncDataLoaderFeature, expandAllFeature],
    });
  }
  const tree = treeRef.current;
  useEffect(() => {
    tree.setMounted(true);
    return () => tree.setMounted(false);
  }, [tree]);
  useEffect(() => {
    tree.setConfig((c) => ({
      ...c,
      isItemFolder: (item) => (children.get(item.getId())?.length ?? 0) > 0,
      dataLoader: {
        getItem: (id) => byId.get(id) ?? null,
        getChildren: (id) => children.get(id) ?? [],
      },
    }));
    tree.applySubStateUpdate("expandedItems", [...children.keys()]);
    tree.rebuildTree();
  }, [tree, byId, children]);

  if (!teams.length) {
    return (
      <EmptyRow>No teams yet. A team groups people and grants them repository access.</EmptyRow>
    );
  }
  if (!visible.length) return <EmptyRow>No team matches “{filter}”.</EmptyRow>;
  return (
    <Tree
      indent={20}
      tree={tree}
      className="py-1 text-sm"
      data-testid="team-tree"
      aria-label="Teams"
    >
      {tree.getItems().map((item) => {
        const t = item.getItemData();
        if (!t) return null;
        const kids = children.get(String(t.id))?.length ?? 0;
        return (
          <TreeItem key={item.getId()} item={item} className="w-full text-left" render={<div />}>
            <TreeItemLabel className="rounded-sm px-2 py-1.5" data-team={t.slug}>
              <HugeiconsIcon
                icon={t.privacy === "secret" ? LockIcon : UserGroupIcon}
                strokeWidth={2}
                className="text-muted-foreground size-4"
              />
              <Link
                to="/orgs/$org/teams/$team"
                params={{ org, team: t.slug }}
                className="text-primary font-semibold hover:underline"
                onClick={(e) => e.stopPropagation()}
              >
                {t.name}
              </Link>
              {t.privacy === "secret" && (
                <Badge variant="outline" size="sm">
                  Secret
                </Badge>
              )}
              <span className="text-muted-foreground hidden truncate text-xs sm:inline">
                {t.description}
              </span>
              {kids > 0 && (
                <span className="text-muted-foreground ml-auto shrink-0 text-xs">
                  {kids} child {kids === 1 ? "team" : "teams"}
                </span>
              )}
            </TreeItemLabel>
          </TreeItem>
        );
      })}
    </Tree>
  );
}

// ---- Create and edit ----

type Values = {
  name: string;
  description: string;
  parent: string;
  privacy: "closed" | "secret";
  notification_setting: "notifications_enabled" | "notifications_disabled";
};

// Teams a team may sit under: not itself, not one of its own descendants, not secret.
function parentChoices(teams: Team[], self: TeamFull | undefined) {
  if (!self) return teams.filter((t) => t.privacy !== "secret");
  const below = new Set<number>([self.id]);
  let grew = true;
  while (grew) {
    grew = false;
    for (const t of teams) {
      if (t.parent && below.has(t.parent.id) && !below.has(t.id)) {
        below.add(t.id);
        grew = true;
      }
    }
  }
  return teams.filter((t) => !below.has(t.id) && t.privacy !== "secret");
}

export function TeamForm({
  org,
  team,
  onDone,
}: {
  org: string;
  team?: TeamFull;
  onDone: () => void;
}) {
  const { data: teams } = useSuspenseQuery(teamsQuery(org));
  const { data: settings } = useSuspenseQuery(teamSettingsQuery(org));
  const navigate = useNavigate();
  const [failure, setFailure] = useState<string>();
  const parents = parentChoices(teams, team);
  const save = useMutation({
    mutationFn: (v: Values) => {
      const body = {
        name: v.name.trim(),
        description: v.description.trim(),
        privacy: v.privacy,
        notification_setting: v.notification_setting,
        parent_team_id: v.parent ? Number(v.parent) : null,
      };
      return team
        ? send(
            api.PATCH("/orgs/{org}/teams/{team_slug}", {
              params: { path: { org, team_slug: team.slug } },
              body,
            }),
          )
        : send(
            api.POST("/orgs/{org}/teams", {
              params: { path: { org } },
              body: { ...body, parent_team_id: body.parent_team_id ?? undefined },
            }),
          );
    },
  });
  const form = useForm({
    defaultValues: {
      name: team?.name ?? "",
      description: team?.description ?? "",
      parent: team?.parent ? String(team.parent.id) : "",
      privacy: (team?.privacy ?? settings.default_privacy) as Values["privacy"],
      notification_setting: (team?.notification_setting ??
        settings.default_notification_setting) as Values["notification_setting"],
    } satisfies Values,
    onSubmit: async ({ value, formApi }) => {
      setFailure(undefined);
      try {
        const saved = await save.mutateAsync(value);
        toast.success(team ? `Saved ${saved.name}` : `Created ${saved.name}`);
        await refetch(teamsQuery(org).queryKey);
        if (team) await refetch(teamQuery({ org, team: team.slug }).queryKey);
        onDone();
        if (!team || saved.slug !== team.slug) {
          void navigate({
            to: "/orgs/$org/teams/$team",
            params: { org, team: saved.slug },
            replace: Boolean(team),
          });
        }
      } catch (e) {
        setFailure(
          applyServerErrors(formApi, e, {
            name: "name",
            parent_team_id: "parent",
            privacy: "privacy",
            maintainers: "name",
          }),
        );
        toast.error(team ? "Team not saved" : "Team not created", {
          description: e instanceof Error ? e.message : String(e),
        });
      }
    },
  });
  return (
    <form
      className="flex flex-col gap-4"
      onSubmit={(e) => {
        e.preventDefault();
        e.stopPropagation();
        void form.handleSubmit();
      }}
    >
      <DialogHeader>
        <DialogTitle>{team ? `Edit ${team.name}` : "Create a team"}</DialogTitle>
        <DialogDescription>
          {team
            ? "A new name changes the team's @mention and its URL."
            : "You become the team's first maintainer."}
        </DialogDescription>
      </DialogHeader>
      <form.Field
        name="name"
        validators={{
          onChange: ({ value }) =>
            !value.trim()
              ? "A team needs a name."
              : !/[a-z0-9]/i.test(value)
                ? "Use at least one letter or digit."
                : value.length > 255
                  ? "At most 255 characters."
                  : undefined,
        }}
      >
        {(field) => (
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="team-name">Team name</Label>
            <Input
              id="team-name"
              value={field.state.value}
              onBlur={field.handleBlur}
              onChange={(e) => field.handleChange(e.target.value)}
              aria-invalid={field.state.meta.errors.length > 0 || undefined}
              autoFocus
            />
            {field.state.value.trim() && (
              <p className="text-muted-foreground text-xs">
                Mention it as{" "}
                <code className="font-mono">
                  @{org}/{slugPreview(field.state.value)}
                </code>
              </p>
            )}
            <FieldMessage field={field} />
          </div>
        )}
      </form.Field>
      <form.Field
        name="description"
        validators={{
          onChange: ({ value }) => (value.length > 500 ? "At most 500 characters." : undefined),
        }}
      >
        {(field) => (
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="team-description">Description</Label>
            <Textarea
              id="team-description"
              rows={2}
              value={field.state.value}
              onChange={(e) => field.handleChange(e.target.value)}
            />
            <FieldMessage field={field} />
          </div>
        )}
      </form.Field>
      <form.Subscribe selector={(s) => s.values.privacy}>
        {(privacy) => (
          <form.Field name="parent">
            {(field) => (
              <div className="flex flex-col gap-1.5">
                <Label htmlFor="team-parent">Parent team</Label>
                <NativeSelect
                  id="team-parent"
                  className="w-full"
                  value={field.state.value}
                  disabled={privacy === "secret"}
                  onChange={(e) => field.handleChange(e.target.value)}
                  aria-invalid={field.state.meta.errors.length > 0 || undefined}
                >
                  <NativeSelectOption value="">No parent</NativeSelectOption>
                  {parents.map((t) => (
                    <NativeSelectOption key={t.id} value={String(t.id)}>
                      {t.name}
                    </NativeSelectOption>
                  ))}
                </NativeSelect>
                <p className="text-muted-foreground text-xs">
                  {privacy === "secret"
                    ? "Secret teams cannot be nested."
                    : "Members of a child team inherit the parent's repository access."}
                </p>
                <FieldMessage field={field} />
              </div>
            )}
          </form.Field>
        )}
      </form.Subscribe>
      <form.Field name="privacy">
        {(field) => (
          <fieldset className="flex flex-col gap-2">
            <legend className="mb-1.5 text-sm font-medium">Visibility</legend>
            <RadioGroup
              value={field.state.value}
              onValueChange={(v) => {
                field.handleChange(v as Values["privacy"]);
                if (v === "secret") form.setFieldValue("parent", "");
              }}
            >
              <label className="flex items-start gap-2 text-sm">
                <RadioGroupItem value="closed" className="mt-0.5" />
                <span>
                  <span className="font-medium">Visible</span>
                  <span className="text-muted-foreground block text-xs">
                    Every member of {org} can see this team and mention it.
                  </span>
                </span>
              </label>
              <label className="flex items-start gap-2 text-sm">
                <RadioGroupItem value="secret" className="mt-0.5" />
                <span>
                  <span className="font-medium">Secret</span>
                  <span className="text-muted-foreground block text-xs">
                    Only its members and owners can see it.
                  </span>
                </span>
              </label>
            </RadioGroup>
            <FieldMessage field={field} />
          </fieldset>
        )}
      </form.Field>
      <form.Field name="notification_setting">
        {(field) => (
          <label className="flex items-center gap-2 text-sm">
            <Checkbox
              checked={field.state.value === "notifications_enabled"}
              onCheckedChange={(on) =>
                field.handleChange(on ? "notifications_enabled" : "notifications_disabled")
              }
            />
            Notify every member when the team is @mentioned
          </label>
        )}
      </form.Field>
      <FormFailure>{failure}</FormFailure>
      <DialogFooter>
        <Button type="button" variant="outline" onClick={onDone}>
          Cancel
        </Button>
        <form.Subscribe selector={(s) => s.isSubmitting}>
          {(submitting) => (
            <Button type="submit" disabled={submitting} data-testid="team-submit">
              {submitting ? "Saving…" : team ? "Save changes" : "Create team"}
            </Button>
          )}
        </form.Subscribe>
      </DialogFooter>
    </form>
  );
}

function slugPreview(name: string) {
  return name
    .toLowerCase()
    .normalize("NFKD")
    .replace(/[\u0300-\u036f]/g, "")
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-|-$/g, "");
}
