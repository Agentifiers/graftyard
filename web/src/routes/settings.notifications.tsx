import { createFileRoute } from "@tanstack/react-router";
import { useMutation, useQueryClient, useSuspenseQuery } from "@tanstack/react-query";
import { useForm } from "@tanstack/react-form";
import { toast } from "sonner";
import { HugeiconsIcon } from "@hugeicons/react";
import { Delete02Icon, PlusSignIcon } from "@hugeicons/core-free-icons";
import { userOrgsQuery } from "@/areas/core/queries";
import { applyServerErrors, failed, FieldErrors, Section, SubmitButton } from "@/areas/account/kit";
import { call, emailsQuery, notificationSettingsQuery } from "@/areas/account/queries";
import { ui, type Channels, type NotificationSettings } from "@/areas/account/ui-api";
import { PageHeader, Panel } from "@/components/panel";
import { Region } from "@/components/region";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Field } from "@/components/ui/field";
import { NativeSelect, NativeSelectOption } from "@/components/ui/native-select";
import { Switch } from "@/components/ui/switch";

export const Route = createFileRoute("/settings/notifications")({
  loader: ({ context: { queryClient } }) => {
    void queryClient.prefetchQuery(notificationSettingsQuery());
    void queryClient.prefetchQuery(emailsQuery());
  },
  component: NotificationsPage,
});

function NotificationsPage() {
  return (
    <div className="flex flex-col gap-6">
      <PageHeader
        title="Notifications"
        description="Where and when Graftyard tells you about activity."
      />
      <Region name="notification settings" rows={8}>
        <NotificationsForm />
      </Region>
    </div>
  );
}

type ChannelKey = "participating" | "watching" | "actions" | "dependabot";

const CHANNELS: { key: ChannelKey; title: string; hint: string }[] = [
  {
    key: "participating",
    title: "Participating, @mentions and custom",
    hint: "Threads you take part in, are mentioned in, or chose to follow.",
  },
  { key: "watching", title: "Watching", hint: "Every conversation in repositories you watch." },
  {
    key: "actions",
    title: "Actions",
    hint: "Workflow runs on repositories you set up Actions for.",
  },
  {
    key: "dependabot",
    title: "Dependabot alerts",
    hint: "Vulnerable dependencies in repositories you can see alerts for.",
  },
];

function NotificationsForm() {
  const queryClient = useQueryClient();
  const { data: settings } = useSuspenseQuery(notificationSettingsQuery());
  const { data: emails } = useSuspenseQuery(emailsQuery());
  const { data: orgs } = useSuspenseQuery(userOrgsQuery());
  const save = useMutation({
    mutationFn: (body: NotificationSettings) => call(ui.PUT("/settings/notifications", { body })),
    onSuccess: (s) => {
      queryClient.setQueryData(notificationSettingsQuery().queryKey, s);
      toast.success("Notification settings saved");
    },
  });
  const form = useForm({
    defaultValues: settings,
    onSubmit: async ({ value, formApi }) => {
      try {
        await save.mutateAsync(value);
        formApi.reset(value);
      } catch (error) {
        if (!applyServerErrors(form, error, { default_email: "default_email", routes: "routes" })) {
          failed("Saving notification settings", error);
        }
      }
    },
  });

  return (
    <form
      noValidate
      className="flex max-w-2xl flex-col gap-8"
      onSubmit={(e) => {
        e.preventDefault();
        void form.handleSubmit();
      }}
      data-testid="notifications-form"
    >
      <Section
        title="Default notifications email"
        description="Where email notifications go, unless an organization routes them elsewhere."
      >
        <form.Field name="default_email">
          {(field) => (
            <Field data-invalid={field.state.meta.errors.length > 0 || undefined}>
              <NativeSelect
                aria-label="Default notifications email"
                value={field.state.value}
                onChange={(e) => {
                  field.setErrorMap({ onServer: undefined });
                  field.handleChange(e.target.value);
                }}
              >
                {emails.map((e) => (
                  <NativeSelectOption key={e.email} value={e.email}>
                    {e.email}
                    {e.verified ? "" : " (unverified)"}
                  </NativeSelectOption>
                ))}
              </NativeSelect>
              <FieldErrors field={field} />
            </Field>
          )}
        </form.Field>
      </Section>

      <Section title="Automatic watching">
        <div className="flex flex-col gap-2">
          <form.Field name="automatically_watch_repositories">
            {(field) => (
              <label className="flex items-center gap-3 text-sm">
                <Switch
                  checked={field.state.value}
                  onCheckedChange={field.handleChange}
                  aria-label="Automatically watch repositories"
                />
                Automatically watch repositories you are given push access to
              </label>
            )}
          </form.Field>
          <form.Field name="automatically_watch_teams">
            {(field) => (
              <label className="flex items-center gap-3 text-sm">
                <Switch
                  checked={field.state.value}
                  onCheckedChange={field.handleChange}
                  aria-label="Automatically watch teams"
                />
                Automatically watch teams you join
              </label>
            )}
          </form.Field>
        </div>
      </Section>

      <Section title="Subscriptions" description="Choose a channel for each kind of notification.">
        <Panel flush>
          <table className="w-full text-sm" data-testid="channels">
            <thead className="bg-muted/40 text-muted-foreground text-xs">
              <tr>
                <th className="px-4 py-2 text-left font-medium">Notify me about</th>
                <th className="w-20 px-2 py-2 font-medium">On Graftyard</th>
                <th className="w-20 px-2 py-2 font-medium">Email</th>
              </tr>
            </thead>
            <tbody className="divide-y">
              {CHANNELS.map((c) => (
                <tr key={c.key} data-channel={c.key}>
                  <td className="px-4 py-2">
                    <p className="font-medium">{c.title}</p>
                    <p className="text-muted-foreground text-xs">{c.hint}</p>
                    {c.key === "actions" && (
                      <form.Field name="actions.failed_only">
                        {(field) => (
                          <label className="mt-1 flex items-center gap-2 text-xs">
                            <Checkbox
                              checked={field.state.value}
                              onCheckedChange={field.handleChange}
                            />
                            Only notify for failed workflows
                          </label>
                        )}
                      </form.Field>
                    )}
                  </td>
                  {(["web", "email"] as (keyof Channels)[]).map((ch) => (
                    <td key={ch} className="px-2 py-2 text-center">
                      <form.Field name={`${c.key}.${ch}`}>
                        {(field) => (
                          <Checkbox
                            checked={Boolean(field.state.value)}
                            onCheckedChange={(on) => field.handleChange(on)}
                            aria-label={`${c.title}: ${ch === "web" ? "on Graftyard" : "email"}`}
                          />
                        )}
                      </form.Field>
                    </td>
                  ))}
                </tr>
              ))}
            </tbody>
          </table>
        </Panel>
        <form.Field name="own_updates">
          {(field) => (
            <label className="flex items-center gap-2 text-sm">
              <Checkbox checked={field.state.value} onCheckedChange={field.handleChange} />
              Include your own updates in email
            </label>
          )}
        </form.Field>
      </Section>

      <Section
        title="Custom routing"
        description="Send an organization's email notifications to another verified address."
      >
        <form.Field name="routes" mode="array">
          {(field) => (
            <div className="flex flex-col gap-2" data-testid="routes">
              {field.state.value.map((_, i) => (
                <div key={i} className="flex items-center gap-2">
                  <form.Field name={`routes[${i}].org`}>
                    {(f) => (
                      <NativeSelect
                        aria-label="Organization"
                        value={f.state.value}
                        onChange={(e) => f.handleChange(e.target.value)}
                      >
                        {orgs.map((o) => (
                          <NativeSelectOption key={o.login} value={o.login}>
                            {o.login}
                          </NativeSelectOption>
                        ))}
                      </NativeSelect>
                    )}
                  </form.Field>
                  <form.Field name={`routes[${i}].email`}>
                    {(f) => (
                      <NativeSelect
                        aria-label="Email"
                        value={f.state.value}
                        onChange={(e) => {
                          field.setErrorMap({ onServer: undefined });
                          f.handleChange(e.target.value);
                        }}
                      >
                        {emails.map((e) => (
                          <NativeSelectOption key={e.email} value={e.email}>
                            {e.email}
                            {e.verified ? "" : " (unverified)"}
                          </NativeSelectOption>
                        ))}
                      </NativeSelect>
                    )}
                  </form.Field>
                  <Button
                    type="button"
                    size="icon-sm"
                    variant="ghost"
                    aria-label="Remove route"
                    onClick={() => field.removeValue(i)}
                  >
                    <HugeiconsIcon icon={Delete02Icon} strokeWidth={2} />
                  </Button>
                </div>
              ))}
              <FieldErrors field={field} />
              {field.state.value.length < orgs.length && (
                <Button
                  type="button"
                  size="sm"
                  variant="outline"
                  className="self-start"
                  onClick={() =>
                    field.pushValue({
                      org:
                        orgs.find((o) => !field.state.value.some((r) => r.org === o.login))
                          ?.login ?? orgs[0].login,
                      email: settings.default_email,
                    })
                  }
                >
                  <HugeiconsIcon icon={PlusSignIcon} strokeWidth={2} data-icon="inline-start" />
                  Add route
                </Button>
              )}
            </div>
          )}
        </form.Field>
      </Section>

      <form.Subscribe selector={(s) => [s.isSubmitting, s.isDirty] as const}>
        {([submitting, dirty]) => (
          <div className="flex items-center gap-3 border-t pt-3">
            <SubmitButton pending={submitting} disabled={submitting || !dirty}>
              Save
            </SubmitButton>
            <span className="text-muted-foreground text-xs">
              {dirty ? "Unsaved changes" : "No unsaved changes"}
            </span>
          </div>
        )}
      </form.Subscribe>
    </form>
  );
}
