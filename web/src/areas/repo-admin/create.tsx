import { useState } from "react";
import { useQuery, useSuspenseQuery } from "@tanstack/react-query";
import { useForm } from "@tanstack/react-form";
import { useNavigate } from "@tanstack/react-router";
import { useDebouncedValue } from "@tanstack/react-pacer";
import { HugeiconsIcon } from "@hugeicons/react";
import { AlertCircleIcon, CheckmarkCircle02Icon } from "@hugeicons/core-free-icons";
import { toast } from "sonner";
import { api } from "@/api/client";
import { userOrgsQuery, userQuery, userReposQuery } from "@/areas/core/queries";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import { NativeSelect, NativeSelectOption } from "@/components/ui/native-select";
import { Spinner } from "@/components/ui/spinner";
import { Textarea } from "@/components/ui/textarea";
import { call, failed, refetch } from "./client";
import { normalizeRepoName } from "./domain";
import { applyServerErrors, FieldMessage } from "./kit";
import { gitignoreTemplatesQuery, licensesQuery, nameTakenQuery } from "./queries";

type Visibility = "public" | "internal" | "private";

// Whether owner/name is free, checked once typing pauses for 400 ms.
export function NameAvailability({ owner, name }: { owner: string; name: string }) {
  const normalized = normalizeRepoName(name);
  const [debounced] = useDebouncedValue(`${owner}/${normalized}`, { wait: 400 });
  const settled = debounced === `${owner}/${normalized}`;
  const [o, n] = debounced.split("/");
  const check = useQuery({ ...nameTakenQuery(o, n), enabled: Boolean(n) && settled });
  if (!normalized) return null;
  if (!settled || check.isFetching) {
    return (
      <span
        className="text-muted-foreground flex items-center gap-1 text-xs"
        data-availability="checking"
      >
        <Spinner className="size-3" /> Checking availability…
      </span>
    );
  }
  if (check.error)
    return <span className="text-muted-foreground text-xs">Could not check the name.</span>;
  return check.data ? (
    <span className="text-destructive flex items-center gap-1 text-xs" data-availability="taken">
      <HugeiconsIcon icon={AlertCircleIcon} strokeWidth={2} className="size-3.5" />
      {owner}/{normalized} already exists.
    </span>
  ) : (
    <span
      className="text-success-foreground dark:text-success flex items-center gap-1 text-xs"
      data-availability="free"
    >
      <HugeiconsIcon icon={CheckmarkCircle02Icon} strokeWidth={2} className="size-3.5" />
      {normalized} is available.
    </span>
  );
}

export function NewRepoForm() {
  const { data: me } = useSuspenseQuery(userQuery());
  const { data: orgs } = useSuspenseQuery(userOrgsQuery());
  const { data: repos } = useSuspenseQuery(userReposQuery());
  // The catalogs only feed two optional selects, so a failed read disables those, not the form.
  const gitignores = useQuery(gitignoreTemplatesQuery());
  const licenses = useQuery(licensesQuery());
  const navigate = useNavigate();
  const [failure, setFailure] = useState<string>();
  const templates = repos.filter((r) => r.is_template);
  const isOrg = (o: string) => orgs.some((x) => x.login === o);

  const form = useForm({
    defaultValues: {
      template: "",
      allBranches: false,
      owner: me.login,
      name: "",
      description: "",
      visibility: "public" as Visibility,
      readme: false,
      gitignore: "",
      license: "",
    },
    onSubmit: async ({ value, formApi }) => {
      setFailure(undefined);
      const name = normalizeRepoName(value.name);
      const org = isOrg(value.owner);
      try {
        const created = value.template
          ? await call(
              api.POST("/repos/{template_owner}/{template_repo}/generate", {
                params: {
                  path: {
                    template_owner: value.template.split("/")[0],
                    template_repo: value.template.split("/")[1],
                  },
                },
                body: {
                  owner: value.owner,
                  name,
                  description: value.description || undefined,
                  private: value.visibility !== "public",
                  include_all_branches: value.allBranches,
                },
              }),
            )
          : org
            ? await call(
                api.POST("/orgs/{org}/repos", {
                  params: { path: { org: value.owner } },
                  body: {
                    name,
                    description: value.description || undefined,
                    visibility: value.visibility,
                    auto_init: value.readme,
                    gitignore_template: value.gitignore || undefined,
                    license_template: value.license || undefined,
                  },
                }),
              )
            : await call(
                api.POST("/user/repos", {
                  body: {
                    name,
                    description: value.description || undefined,
                    private: value.visibility !== "public",
                    auto_init: value.readme,
                    gitignore_template: value.gitignore || undefined,
                    license_template: value.license || undefined,
                  },
                }),
              );
        toast.success(`Created ${created.full_name}`);
        await refetch(["user", "repos"]);
        void navigate({
          to: "/$owner/$repo",
          params: { owner: created.owner.login, repo: created.name },
        });
      } catch (e) {
        setFailure(
          applyServerErrors(formApi, e, {
            name: "name",
            visibility: "visibility",
            template: "template",
          }),
        );
        failed("Repository not created")(e);
      }
    },
  });

  return (
    <form
      className="flex flex-col gap-5"
      onSubmit={(e) => {
        e.preventDefault();
        void form.handleSubmit();
      }}
      data-testid="new-repo-form"
    >
      <form.Field name="template">
        {(field) => (
          <label className="flex max-w-md flex-col gap-1 text-sm font-medium">
            Repository template
            <NativeSelect
              value={field.state.value}
              onChange={(e) => field.handleChange(e.target.value)}
              className="w-full"
            >
              <NativeSelectOption value="">No template</NativeSelectOption>
              {templates.map((t) => (
                <NativeSelectOption key={t.full_name} value={t.full_name}>
                  {t.full_name}
                </NativeSelectOption>
              ))}
            </NativeSelect>
            <span className="text-muted-foreground text-xs font-normal">
              {templates.length
                ? "Start with a template repository's files and branches."
                : "No template repositories yet. Mark one in its General settings."}
            </span>
          </label>
        )}
      </form.Field>
      <form.Subscribe selector={(s) => s.values.template}>
        {(t) =>
          t && (
            <form.Field name="allBranches">
              {(field) => (
                <label className="flex items-center gap-2 text-sm">
                  <Checkbox
                    checked={field.state.value}
                    onCheckedChange={(v) => field.handleChange(Boolean(v))}
                  />
                  Include all branches
                </label>
              )}
            </form.Field>
          )
        }
      </form.Subscribe>

      <div className="flex flex-wrap items-start gap-2">
        <form.Field name="owner">
          {(field) => (
            <label className="flex flex-col gap-1 text-sm font-medium">
              Owner
              <NativeSelect
                aria-label="Owner"
                value={field.state.value}
                onChange={(e) => {
                  field.handleChange(e.target.value);
                  // An org here forbids public repositories; a user cannot own an internal one.
                  const v = form.getFieldValue("visibility");
                  if (isOrg(e.target.value) && v === "public")
                    form.setFieldValue("visibility", "private");
                  if (!isOrg(e.target.value) && v === "internal")
                    form.setFieldValue("visibility", "private");
                }}
              >
                <NativeSelectOption value={me.login}>{me.login}</NativeSelectOption>
                {orgs.map((o) => (
                  <NativeSelectOption key={o.login} value={o.login}>
                    {o.login}
                  </NativeSelectOption>
                ))}
              </NativeSelect>
            </label>
          )}
        </form.Field>
        <span className="text-muted-foreground pt-7 text-lg">/</span>
        <form.Field
          name="name"
          validators={{
            onChange: ({ value }) =>
              !value.trim()
                ? "A repository name is required."
                : value.length > 100
                  ? "At most 100 characters."
                  : undefined,
          }}
        >
          {(field) => (
            <div className="flex min-w-64 flex-1 flex-col gap-1">
              <label htmlFor="new-repo-name" className="text-sm font-medium">
                Repository name
              </label>
              <Input
                id="new-repo-name"
                autoFocus
                value={field.state.value}
                onChange={(e) => field.handleChange(e.target.value)}
                aria-invalid={field.state.meta.errors.length > 0 || undefined}
              />
              <form.Subscribe selector={(s) => s.values.owner}>
                {(owner) => <NameAvailability owner={owner} name={field.state.value} />}
              </form.Subscribe>
              {field.state.value.trim() &&
                normalizeRepoName(field.state.value) !== field.state.value.trim() && (
                  <span className="text-muted-foreground text-xs">
                    Your new repository will be created as{" "}
                    <strong>{normalizeRepoName(field.state.value)}</strong>.
                  </span>
                )}
              <FieldMessage field={field} />
            </div>
          )}
        </form.Field>
      </div>

      <form.Field
        name="description"
        validators={{
          onChange: ({ value }) => (value.length > 350 ? "At most 350 characters." : undefined),
        }}
      >
        {(field) => (
          <label className="flex max-w-xl flex-col gap-1 text-sm font-medium">
            Description{" "}
            <span className="text-muted-foreground text-xs font-normal">(optional)</span>
            <Textarea
              rows={2}
              value={field.state.value}
              onChange={(e) => field.handleChange(e.target.value)}
            />
            <FieldMessage field={field} />
          </label>
        )}
      </form.Field>

      <form.Subscribe selector={(s) => s.values.owner}>
        {(owner) => (
          <form.Field name="visibility">
            {(field) => (
              <fieldset className="flex flex-col gap-2 rounded-sm border p-3">
                <legend className="px-1 text-sm font-medium">Visibility</legend>
                {(
                  [
                    [
                      "public",
                      "Public",
                      "Anyone on this instance can see this repository. You choose who can commit.",
                    ],
                    ["internal", "Internal", "Members of the enterprise can see this repository."],
                    ["private", "Private", "You choose who can see and commit to this repository."],
                  ] as const
                ).map(([v, label, description]) => {
                  const disabled =
                    (v === "internal" && !isOrg(owner)) || (v === "public" && isOrg(owner));
                  return (
                    <label
                      key={v}
                      className="flex items-start gap-2 text-sm data-disabled:opacity-50"
                      data-disabled={disabled || undefined}
                    >
                      <input
                        type="radio"
                        name="visibility"
                        className="mt-1"
                        value={v}
                        checked={field.state.value === v}
                        disabled={disabled}
                        onChange={() => field.handleChange(v)}
                      />
                      <span>
                        <span className="font-medium">{label}</span>
                        <span className="text-muted-foreground block text-xs">
                          {description}
                          {disabled &&
                            v === "public" &&
                            " The organization does not allow members to create public repositories."}
                          {disabled &&
                            v === "internal" &&
                            " Only organization-owned repositories can be internal."}
                        </span>
                      </span>
                    </label>
                  );
                })}
                <FieldMessage field={field} />
              </fieldset>
            )}
          </form.Field>
        )}
      </form.Subscribe>

      <form.Subscribe selector={(s) => s.values.template}>
        {(template) =>
          !template && (
            <fieldset className="flex flex-col gap-3 rounded-sm border p-3">
              <legend className="px-1 text-sm font-medium">Initialize this repository with</legend>
              <form.Field name="readme">
                {(field) => (
                  <label className="flex items-start gap-2 text-sm">
                    <Checkbox
                      className="mt-0.5"
                      checked={field.state.value}
                      onCheckedChange={(v) => field.handleChange(Boolean(v))}
                    />
                    <span>
                      <span className="font-medium">Add a README file</span>
                      <span className="text-muted-foreground block text-xs">
                        A long description for the project.
                      </span>
                    </span>
                  </label>
                )}
              </form.Field>
              <form.Field name="gitignore">
                {(field) => (
                  <label className="flex max-w-xs flex-col gap-1 text-sm font-medium">
                    .gitignore template
                    <NativeSelect
                      value={field.state.value}
                      onChange={(e) => field.handleChange(e.target.value)}
                      className="w-full"
                      disabled={!gitignores.data}
                    >
                      <NativeSelectOption value="">None</NativeSelectOption>
                      {(gitignores.data ?? []).map((g) => (
                        <NativeSelectOption key={g} value={g}>
                          {g}
                        </NativeSelectOption>
                      ))}
                    </NativeSelect>
                    <CatalogError what=".gitignore templates" query={gitignores} />
                  </label>
                )}
              </form.Field>
              <form.Field name="license">
                {(field) => (
                  <label className="flex max-w-xs flex-col gap-1 text-sm font-medium">
                    License
                    <NativeSelect
                      value={field.state.value}
                      onChange={(e) => field.handleChange(e.target.value)}
                      className="w-full"
                      disabled={!licenses.data}
                    >
                      <NativeSelectOption value="">None</NativeSelectOption>
                      {(licenses.data ?? []).map((l) => (
                        <NativeSelectOption key={l.key} value={l.key}>
                          {l.name}
                        </NativeSelectOption>
                      ))}
                    </NativeSelect>
                    <CatalogError what="licenses" query={licenses} />
                  </label>
                )}
              </form.Field>
              <form.Subscribe
                selector={(s) =>
                  s.values.readme || Boolean(s.values.gitignore) || Boolean(s.values.license)
                }
              >
                {(init) => (
                  <p className="text-muted-foreground text-xs">
                    {init
                      ? "This sets main as the default branch."
                      : "Leave everything off to import an existing repository."}
                  </p>
                )}
              </form.Subscribe>
            </fieldset>
          )
        }
      </form.Subscribe>

      {failure && (
        <p className="text-destructive text-sm" role="alert">
          {failure}
        </p>
      )}
      <form.Subscribe selector={(s) => s.isSubmitting}>
        {(submitting) => (
          <div>
            <Button type="submit" disabled={submitting}>
              {submitting ? "Creating…" : "Create repository"}
            </Button>
          </div>
        )}
      </form.Subscribe>
    </form>
  );
}

function CatalogError({
  what,
  query,
}: {
  what: string;
  query: { error: Error | null; refetch: () => unknown };
}) {
  if (!query.error) return null;
  return (
    <span className="text-destructive text-xs font-normal" role="alert">
      Could not load {what}: {query.error.message}.{" "}
      <button type="button" className="underline" onClick={() => void query.refetch()}>
        Try again
      </button>
    </span>
  );
}
