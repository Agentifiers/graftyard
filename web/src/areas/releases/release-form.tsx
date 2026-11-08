import { useState } from "react";
import { useForm } from "@tanstack/react-form";
import { useMutation, useQueryClient, useSuspenseQuery } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { HugeiconsIcon } from "@hugeicons/react";
import { MagicWand01Icon, Tag01Icon } from "@hugeicons/core-free-icons";
import { toast } from "sonner";
import { api, type Schemas } from "@/api/client";
import { MarkdownBody } from "@/components/markdown";
import { Panel } from "@/components/panel";
import { Alert, AlertDescription, AlertTitle } from "@/components/reui/alert";
import {
  Autocomplete,
  AutocompleteContent,
  AutocompleteEmpty,
  AutocompleteInput,
  AutocompleteItem,
  AutocompleteList,
} from "@/components/reui/autocomplete";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Field, FieldDescription, FieldError, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { NativeSelect, NativeSelectOption } from "@/components/ui/native-select";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { Textarea } from "@/components/ui/textarea";
import { errorText, send, serverError, WriteError } from "./api";
import { AssetList } from "./parts";
import { allReleasesQuery, branchesQuery, releasesKey, tagsQuery } from "./queries";
import { AssetDropzone, useAssetUploads } from "./uploads";
import { useOptimisticWrite } from "./write";

type Repo = { owner: string; repo: string };
type Release = Schemas["release"];

type Values = {
  tag: string;
  target: string;
  previousTag: string;
  name: string;
  body: string;
  prerelease: boolean;
  makeLatest: boolean;
};

// GHES's field names, mapped to this form's.
const FIELDS: Record<string, keyof Values> = {
  tag_name: "tag",
  target_commitish: "target",
  name: "name",
  body: "body",
};

// Rejects what git rejects in a ref name, so most bad tags never reach the server.
function tagProblem(tag: string) {
  if (!tag.trim()) return "Choose or create a tag.";
  if (/\s/.test(tag)) return "A tag cannot contain spaces.";
  if (/\.\.|[~^:?*[\\]|\.lock$|^[-/.]|\/$/.test(tag)) return "This is not a valid tag name.";
  return undefined;
}

// The release editor. Without `release` it drafts a new one; with it, it edits that one.
// "Publish" and "Save draft" share the form and differ only in the draft flag they send.
export function ReleaseForm({
  release,
  latest,
  ...repo
}: Repo & { release?: Release; latest?: boolean }) {
  const queryClient = useQueryClient();
  const navigate = useNavigate();
  const { data: tags } = useSuspenseQuery(tagsQuery(repo));
  const { data: branches } = useSuspenseQuery(branchesQuery(repo));
  const { data: releases } = useSuspenseQuery(allReleasesQuery(repo));
  const up = useAssetUploads(repo);
  const [intent, setIntent] = useState<"publish" | "draft">("publish");
  const [failure, setFailure] = useState<string>();
  const tagNames = tags.map((t) => t.name);

  const form = useForm({
    defaultValues: {
      tag: release?.tag_name ?? "",
      target: release?.target_commitish ?? "main",
      previousTag: "",
      name: release?.name ?? "",
      body: release?.body ?? "",
      prerelease: release?.prerelease ?? false,
      makeLatest: release ? Boolean(latest) : true,
    } satisfies Values,
    onSubmit: async ({ value, formApi }) => {
      setFailure(undefined);
      const draft = intent === "draft";
      const fields = {
        tag_name: value.tag.trim(),
        target_commitish: value.target,
        name: value.name.trim() || value.tag.trim(),
        body: value.body,
        prerelease: value.prerelease,
        make_latest: value.makeLatest && !value.prerelease ? ("true" as const) : ("false" as const),
      };
      try {
        let saved: Release;
        if (release) {
          saved = await send(
            api.PATCH("/repos/{owner}/{repo}/releases/{release_id}", {
              params: { path: { ...repo, release_id: release.id } },
              body: { ...fields, draft },
            }),
          );
        } else {
          // Files wait for a release to attach to: create it as a draft, upload, then publish.
          const holdForUploads = up.items.length > 0 && !draft;
          saved = await send(
            api.POST("/repos/{owner}/{repo}/releases", {
              params: { path: repo },
              body: { ...fields, draft: draft || holdForUploads },
            }),
          );
          if (up.items.length > 0) {
            const ok = await up.sendAll(saved.id);
            if (!ok) {
              await queryClient.invalidateQueries({ queryKey: releasesKey(repo) });
              toast.error("Some files did not upload", {
                description: "The release was saved as a draft. Fix the files and publish.",
              });
              void navigate({
                to: "/$owner/$repo/releases/edit/$tag",
                params: { ...repo, tag: saved.tag_name },
              });
              return;
            }
            if (holdForUploads) {
              saved = await send(
                api.PATCH("/repos/{owner}/{repo}/releases/{release_id}", {
                  params: { path: { ...repo, release_id: saved.id } },
                  body: { draft: false, make_latest: fields.make_latest },
                }),
              );
            }
          }
        }
        await queryClient.invalidateQueries({ queryKey: releasesKey(repo) });
        await queryClient.invalidateQueries({ queryKey: ["repos", repo.owner, repo.repo, "tags"] });
        toast.success(
          saved.draft
            ? `Saved draft ${saved.name || saved.tag_name}`
            : `Published ${saved.name || saved.tag_name}`,
        );
        if (saved.draft) void navigate({ to: "/$owner/$repo/releases", params: repo });
        else
          void navigate({
            to: "/$owner/$repo/releases/tag/$tag",
            params: { ...repo, tag: saved.tag_name },
          });
      } catch (e) {
        if (e instanceof WriteError && Object.keys(e.fields).length > 0) {
          for (const [field, message] of Object.entries(e.fields)) {
            const name = FIELDS[field];
            if (name) formApi.setFieldMeta(name, serverError(`${labelOf(name)} ${message}.`));
          }
          setFailure("The server rejected some fields. Fix them and try again.");
        } else {
          setFailure(errorText(e));
        }
        toast.error(release ? "Release not saved" : "Release not created", {
          description: errorText(e),
        });
      }
    },
  });

  const generate = useMutation({
    mutationFn: (v: Pick<Values, "tag" | "target" | "previousTag">) =>
      send(
        api.POST("/repos/{owner}/{repo}/releases/generate-notes", {
          params: { path: repo },
          body: {
            tag_name: v.tag.trim(),
            target_commitish: v.target,
            previous_tag_name: v.previousTag || undefined,
          },
        }),
      ),
    onSuccess: (notes) => {
      if (!form.getFieldValue("name").trim()) form.setFieldValue("name", notes.name);
      const current = form.getFieldValue("body").trim();
      form.setFieldValue("body", current ? `${current}\n\n${notes.body}` : notes.body);
      toast.success("Release notes generated");
    },
    onError: (e) => toast.error("Could not generate release notes", { description: errorText(e) }),
  });

  return (
    <form
      className="grid items-start gap-4 lg:grid-cols-[minmax(0,1fr)_18rem]"
      data-testid="release-form"
      noValidate
      onSubmit={(e) => {
        e.preventDefault();
        void form.handleSubmit();
      }}
    >
      <div className="flex min-w-0 flex-col gap-4">
        <Panel title="Tag and target">
          <div className="grid gap-4 sm:grid-cols-2">
            <form.Field
              name="tag"
              validators={{
                onChange: ({ value }) => (value ? tagProblem(value) : undefined),
                onSubmit: ({ value }) =>
                  tagProblem(value) ??
                  (releases.some((r) => r.tag_name === value.trim() && r.id !== release?.id)
                    ? "A release already uses this tag."
                    : undefined),
              }}
            >
              {(field) => {
                const value = field.state.value.trim();
                const exists = tagNames.includes(value);
                const errors = field.state.meta.errors.filter(Boolean).map(String);
                return (
                  <Field data-invalid={errors.length > 0 || undefined}>
                    <FieldLabel htmlFor="release-tag">Tag</FieldLabel>
                    <Autocomplete
                      items={tagNames}
                      value={field.state.value}
                      onValueChange={(v) => field.handleChange(v)}
                      openOnInputClick
                    >
                      <AutocompleteInput
                        id="release-tag"
                        name="tag"
                        placeholder="Choose or create a tag, e.g. v1.0.0"
                        autoComplete="off"
                        aria-invalid={errors.length > 0 || undefined}
                        onBlur={field.handleBlur}
                        showTrigger
                      />
                      <AutocompleteContent>
                        <AutocompleteEmpty>
                          {value ? `Create new tag ${value} on publish` : "No tags yet"}
                        </AutocompleteEmpty>
                        <AutocompleteList>
                          {(t: string) => (
                            <AutocompleteItem key={t} value={t}>
                              <HugeiconsIcon icon={Tag01Icon} strokeWidth={2} />
                              {t}
                            </AutocompleteItem>
                          )}
                        </AutocompleteList>
                      </AutocompleteContent>
                    </Autocomplete>
                    {errors.length > 0 ? (
                      <FieldError>{errors.join(" ")}</FieldError>
                    ) : (
                      <FieldDescription data-testid="tag-status">
                        {!value
                          ? "Pick an existing tag or type a new one."
                          : exists
                            ? "Existing tag."
                            : "New tag. It is created from the target when you publish."}
                      </FieldDescription>
                    )}
                  </Field>
                );
              }}
            </form.Field>
            <form.Subscribe selector={(s) => s.values.tag.trim()}>
              {(tag) => (
                <form.Field name="target">
                  {(field) => {
                    const errors = field.state.meta.errors.filter(Boolean).map(String);
                    const locked = tagNames.includes(tag);
                    return (
                      <Field data-invalid={errors.length > 0 || undefined}>
                        <FieldLabel htmlFor="release-target">Target</FieldLabel>
                        <NativeSelect
                          id="release-target"
                          className="w-full"
                          value={field.state.value}
                          disabled={locked}
                          onChange={(e) => field.handleChange(e.target.value)}
                        >
                          {branches.map((b) => (
                            <NativeSelectOption key={b.name} value={b.name}>
                              {b.name}
                            </NativeSelectOption>
                          ))}
                        </NativeSelect>
                        {errors.length > 0 ? (
                          <FieldError>{errors.join(" ")}</FieldError>
                        ) : (
                          <FieldDescription>
                            {locked
                              ? "The tag already points at a commit."
                              : "The branch the new tag is created from."}
                          </FieldDescription>
                        )}
                      </Field>
                    );
                  }}
                </form.Field>
              )}
            </form.Subscribe>
          </div>
        </Panel>

        <Panel
          title="Release notes"
          actions={
            <form.Subscribe selector={(s) => s.values}>
              {(v) => (
                <div className="flex items-center gap-2">
                  <NativeSelect
                    size="sm"
                    aria-label="Previous tag"
                    value={v.previousTag}
                    onChange={(e) => form.setFieldValue("previousTag", e.target.value)}
                  >
                    <NativeSelectOption value="">Previous tag: auto</NativeSelectOption>
                    {tagNames
                      .filter((t) => t !== v.tag.trim())
                      .map((t) => (
                        <NativeSelectOption key={t} value={t}>
                          Previous tag: {t}
                        </NativeSelectOption>
                      ))}
                  </NativeSelect>
                  <Button
                    type="button"
                    size="sm"
                    variant="outline"
                    disabled={Boolean(tagProblem(v.tag)) || generate.isPending}
                    onClick={() => generate.mutate(v)}
                  >
                    <HugeiconsIcon
                      icon={MagicWand01Icon}
                      strokeWidth={2}
                      data-icon="inline-start"
                    />
                    {generate.isPending ? "Generating…" : "Generate release notes"}
                  </Button>
                </div>
              )}
            </form.Subscribe>
          }
        >
          <div className="flex flex-col gap-3">
            <form.Field
              name="name"
              validators={{
                onChange: ({ value }) =>
                  value.length > 255 ? "Keep the title under 256 characters." : undefined,
              }}
            >
              {(field) => {
                const errors = field.state.meta.errors.filter(Boolean).map(String);
                return (
                  <Field data-invalid={errors.length > 0 || undefined}>
                    <FieldLabel htmlFor="release-title">Release title</FieldLabel>
                    <Input
                      id="release-title"
                      name="name"
                      placeholder="Defaults to the tag"
                      value={field.state.value}
                      onBlur={field.handleBlur}
                      onChange={(e) => field.handleChange(e.target.value)}
                    />
                    {errors.length > 0 && <FieldError>{errors.join(" ")}</FieldError>}
                  </Field>
                );
              }}
            </form.Field>
            <form.Field name="body">
              {(field) => (
                <Tabs defaultValue="write">
                  <TabsList variant="line">
                    <TabsTrigger value="write">Write</TabsTrigger>
                    <TabsTrigger value="preview">Preview</TabsTrigger>
                  </TabsList>
                  <TabsContent value="write">
                    <Textarea
                      aria-label="Release notes"
                      name="body"
                      className="min-h-56 font-mono text-xs"
                      placeholder="Describe this release. Markdown works."
                      value={field.state.value}
                      onBlur={field.handleBlur}
                      onChange={(e) => field.handleChange(e.target.value)}
                    />
                  </TabsContent>
                  <TabsContent
                    value="preview"
                    className="min-h-56 border p-3"
                    data-testid="preview"
                  >
                    {field.state.value.trim() ? (
                      <MarkdownBody text={field.state.value} />
                    ) : (
                      <p className="text-muted-foreground text-sm">Nothing to preview.</p>
                    )}
                  </TabsContent>
                </Tabs>
              )}
            </form.Field>
          </div>
        </Panel>

        <Panel title="Assets">
          <div className="flex flex-col gap-3">
            {release && release.assets.length > 0 && <ExistingAssets {...repo} release={release} />}
            <AssetDropzone uploads={up} releaseId={release?.id} />
          </div>
        </Panel>
      </div>

      <div className="flex flex-col gap-4 lg:sticky lg:top-4">
        <Panel title="Options">
          <div className="flex flex-col gap-3">
            <form.Field name="prerelease">
              {(field) => (
                <Field orientation="horizontal">
                  <Checkbox
                    id="release-prerelease"
                    checked={field.state.value}
                    onCheckedChange={(v) => {
                      field.handleChange(v);
                      if (v) form.setFieldValue("makeLatest", false);
                    }}
                  />
                  <FieldLabel htmlFor="release-prerelease" className="flex-col items-start gap-0.5">
                    Set as a pre-release
                    <span className="text-muted-foreground text-xs font-normal">
                      Not ready for production.
                    </span>
                  </FieldLabel>
                </Field>
              )}
            </form.Field>
            <form.Subscribe selector={(s) => s.values.prerelease}>
              {(prerelease) => (
                <form.Field name="makeLatest">
                  {(field) => (
                    <Field orientation="horizontal">
                      <Checkbox
                        id="release-latest"
                        checked={field.state.value && !prerelease}
                        disabled={prerelease}
                        onCheckedChange={(v) => field.handleChange(v)}
                      />
                      <FieldLabel htmlFor="release-latest" className="flex-col items-start gap-0.5">
                        Set as the latest release
                        <span className="text-muted-foreground text-xs font-normal">
                          {prerelease
                            ? "A pre-release cannot be the latest."
                            : "Shown on the repository's front page."}
                        </span>
                      </FieldLabel>
                    </Field>
                  )}
                </form.Field>
              )}
            </form.Subscribe>
          </div>
        </Panel>
        {failure && (
          <Alert variant="destructive" data-testid="form-failure">
            <AlertTitle>{release ? "Release not saved" : "Release not created"}</AlertTitle>
            <AlertDescription>{failure}</AlertDescription>
          </Alert>
        )}
        <form.Subscribe selector={(s) => s.isSubmitting}>
          {(submitting) => (
            <div className="flex flex-col gap-2">
              <Button
                type="submit"
                disabled={submitting || up.busy}
                onClick={() => setIntent("publish")}
              >
                {submitting && intent === "publish"
                  ? "Publishing…"
                  : release && !release.draft
                    ? "Update release"
                    : "Publish release"}
              </Button>
              <Button
                type="submit"
                variant="outline"
                disabled={submitting || up.busy}
                onClick={() => setIntent("draft")}
              >
                {submitting && intent === "draft"
                  ? "Saving…"
                  : release && !release.draft
                    ? "Convert to draft"
                    : "Save draft"}
              </Button>
            </div>
          )}
        </form.Subscribe>
      </div>
    </form>
  );
}

const labelOf = (name: keyof Values) =>
  ({ tag: "Tag", target: "Target", name: "Title", body: "Notes" })[name as string] ?? name;

// The assets a saved release already has, each deletable. The row leaves at once and comes
// back if the delete fails.
function ExistingAssets({ release, ...repo }: Repo & { release: Release }) {
  const key = allReleasesQuery(repo).queryKey;
  const remove = useOptimisticWrite({
    key,
    invalidate: [releasesKey(repo)],
    failure: "Asset not deleted",
    apply: (list: Release[], asset: Schemas["release-asset"]) =>
      list.map((r) =>
        r.id === release.id ? { ...r, assets: r.assets.filter((a) => a.id !== asset.id) } : r,
      ),
    write: (asset: Schemas["release-asset"]) =>
      send(
        api.DELETE("/repos/{owner}/{repo}/releases/assets/{asset_id}", {
          params: { path: { ...repo, asset_id: asset.id } },
        }),
      ),
    onSuccess: (_r, asset) => toast.success(`Deleted ${asset.name}`),
  });
  return (
    <AssetList {...repo} release={{ ...release, draft: true }} onDelete={(a) => remove.mutate(a)} />
  );
}
