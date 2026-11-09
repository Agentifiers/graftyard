import { createFileRoute } from "@tanstack/react-router";
import { useSuspenseQuery } from "@tanstack/react-query";
import { useForm } from "@tanstack/react-form";
import { HugeiconsIcon } from "@hugeicons/react";
import { Cancel01Icon, Megaphone01Icon } from "@hugeicons/core-free-icons";
import { api } from "@/api/client";
import { announcementQuery } from "@/areas/admin/queries";
import { send, showFieldErrors, TextField, useWrite, when } from "@/areas/admin/ui";
import { MarkdownBody } from "@/components/markdown";
import { PageHeader, Panel } from "@/components/panel";
import { Region } from "@/components/region";
import { Button } from "@/components/ui/button";
import { Field, FieldDescription, FieldLabel } from "@/components/ui/field";
import { Switch } from "@/components/ui/switch";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";

export const Route = createFileRoute("/stafftools/announcement")({
  loader: ({ context: { queryClient } }) => void queryClient.prefetchQuery(announcementQuery()),
  component: () => (
    <>
      <PageHeader
        title="Announcement"
        description="A banner every signed-in user sees at the top of each page until it expires."
      />
      <Region name="announcement" rows={6}>
        <AnnouncementForm />
      </Region>
    </>
  ),
});

const KEY = announcementQuery().queryKey;
const MAX = 1024;

// datetime-local wants "YYYY-MM-DDTHH:mm" in local time.
const toLocal = (iso: string | null | undefined) => {
  if (!iso) return "";
  const d = new Date(iso);
  return new Date(d.getTime() - d.getTimezoneOffset() * 60000).toISOString().slice(0, 16);
};

function AnnouncementForm() {
  const { data } = useSuspenseQuery(announcementQuery());
  const save = useWrite({
    run: (body: { announcement: string; expires_at: string | null; user_dismissible: boolean }) =>
      send(api.PATCH("/enterprise/announcement", { body })),
    invalidate: [KEY],
    success: "Announcement published",
    onError: (e) =>
      showFieldErrors(e, form, { announcement: "announcement", expires_at: "expires_at" }),
  });
  const clear = useWrite({
    run: () => send(api.DELETE("/enterprise/announcement")),
    invalidate: [KEY],
    success: "Announcement cleared",
    onSuccess: () => form.reset({ announcement: "", expires_at: "", user_dismissible: false }),
  });
  const form = useForm({
    defaultValues: {
      announcement: data.announcement ?? "",
      expires_at: toLocal(data.expires_at),
      user_dismissible: data.user_dismissible ?? false,
    },
    onSubmit: ({ value }) =>
      save
        .mutateAsync({
          announcement: value.announcement.trim(),
          expires_at: value.expires_at ? new Date(value.expires_at).toISOString() : null,
          user_dismissible: value.user_dismissible,
        })
        .catch(() => {}),
  });

  return (
    <div className="flex flex-col gap-4">
      <Panel title="Live banner">
        {data.announcement ? (
          <Banner
            text={data.announcement}
            dismissible={!!data.user_dismissible}
            expires={data.expires_at ?? null}
          />
        ) : (
          <p className="text-muted-foreground text-sm" data-testid="no-announcement">
            No announcement is set.
          </p>
        )}
      </Panel>
      <Panel
        title="Edit"
        actions={
          data.announcement && (
            <Button
              size="sm"
              variant="destructive"
              disabled={clear.isPending}
              onClick={() => clear.mutate(undefined)}
            >
              <HugeiconsIcon icon={Cancel01Icon} strokeWidth={2} data-icon="inline-start" />
              Clear announcement
            </Button>
          )
        }
      >
        <form
          className="flex flex-col gap-4"
          onSubmit={(e) => {
            e.preventDefault();
            void form.handleSubmit();
          }}
        >
          <form.Field
            name="announcement"
            validators={{
              onSubmit: ({ value }) =>
                !value.trim()
                  ? "Write the announcement, or clear it instead."
                  : value.length > MAX
                    ? `At most ${MAX} characters.`
                    : undefined,
            }}
          >
            {(field) => (
              <Tabs defaultValue="write">
                <TabsList variant="line">
                  <TabsTrigger value="write">Write</TabsTrigger>
                  <TabsTrigger value="preview">Preview</TabsTrigger>
                </TabsList>
                <TabsContent value="write">
                  <TextField
                    field={field}
                    label="Message (Markdown)"
                    multiline
                    description={`${field.state.value.length} of ${MAX} characters. Links, bold, and code render.`}
                  />
                </TabsContent>
                <TabsContent value="preview">
                  <div className="min-h-28" data-testid="announcement-preview">
                    {field.state.value.trim() ? (
                      <Banner
                        text={field.state.value}
                        dismissible={form.getFieldValue("user_dismissible")}
                        expires={
                          form.getFieldValue("expires_at")
                            ? new Date(form.getFieldValue("expires_at")).toISOString()
                            : null
                        }
                      />
                    ) : (
                      <p className="text-muted-foreground text-sm">Nothing to preview.</p>
                    )}
                  </div>
                </TabsContent>
              </Tabs>
            )}
          </form.Field>
          <div className="grid gap-4 sm:grid-cols-2">
            <form.Field
              name="expires_at"
              validators={{
                onSubmit: ({ value }) =>
                  value && new Date(value).getTime() <= Date.now()
                    ? "Pick a time in the future."
                    : undefined,
              }}
            >
              {(field) => (
                <TextField
                  field={field}
                  type="datetime-local"
                  label="Expires"
                  description="Leave empty to keep it until you clear it."
                />
              )}
            </form.Field>
            <form.Field name="user_dismissible">
              {(field) => (
                <Field orientation="horizontal" className="items-start pt-6">
                  <Switch
                    id="dismissible"
                    checked={field.state.value}
                    onCheckedChange={(v) => field.handleChange(v)}
                  />
                  <div>
                    <FieldLabel htmlFor="dismissible">Users can dismiss it</FieldLabel>
                    <FieldDescription>
                      A dismissed banner stays hidden for that user until it changes.
                    </FieldDescription>
                  </div>
                </Field>
              )}
            </form.Field>
          </div>
          <div className="flex justify-end">
            <form.Subscribe selector={(s) => s.isSubmitting}>
              {(busy) => (
                <Button type="submit" size="sm" disabled={busy}>
                  {busy ? "Publishing" : "Publish announcement"}
                </Button>
              )}
            </form.Subscribe>
          </div>
        </form>
      </Panel>
    </div>
  );
}

function Banner({
  text,
  dismissible,
  expires,
}: {
  text: string;
  dismissible: boolean;
  expires: string | null;
}) {
  return (
    <div
      className="border-warning/40 bg-warning/10 flex items-start gap-3 rounded-sm border px-3 py-2"
      data-testid="banner"
    >
      <HugeiconsIcon
        icon={Megaphone01Icon}
        strokeWidth={2}
        className="text-warning-foreground dark:text-warning mt-1.5 size-4 shrink-0"
      />
      <div className="min-w-0 flex-1">
        <MarkdownBody text={text} className="prose-p:my-1" />
        <p className="text-muted-foreground text-xs">
          {expires ? `Until ${when(expires)}` : "No expiry"}
        </p>
      </div>
      {dismissible && (
        <span className="text-muted-foreground mt-1 text-xs" aria-label="Dismissible">
          <HugeiconsIcon icon={Cancel01Icon} strokeWidth={2} className="size-4" />
        </span>
      )}
    </div>
  );
}
