import { createFileRoute } from "@tanstack/react-router";
import { useMutation, useQueryClient, useSuspenseQuery } from "@tanstack/react-query";
import { useForm } from "@tanstack/react-form";
import { toast } from "sonner";
import { HugeiconsIcon } from "@hugeicons/react";
import { Delete02Icon, Mail01Icon, SentIcon } from "@hugeicons/core-free-icons";
import { api, type Schemas } from "@/api/client";
import { userQuery } from "@/areas/core/queries";
import {
  applyServerErrors,
  Confirm,
  failed,
  Section,
  SubmitButton,
  TextField,
  useOptimistic,
} from "@/areas/account/kit";
import { call, emailsQuery, emailVisibilityQuery } from "@/areas/account/queries";
import { ui, type EmailVisibility } from "@/areas/account/ui-api";
import { PageHeader, Panel } from "@/components/panel";
import { Region } from "@/components/region";
import { Badge } from "@/components/reui/badge";
import { Button } from "@/components/ui/button";
import { Field, FieldContent, FieldDescription, FieldLabel } from "@/components/ui/field";
import { NativeSelect, NativeSelectOption } from "@/components/ui/native-select";
import { Switch } from "@/components/ui/switch";

export const Route = createFileRoute("/settings/emails")({
  loader: ({ context: { queryClient } }) => {
    void queryClient.prefetchQuery(emailsQuery());
    void queryClient.prefetchQuery(emailVisibilityQuery());
  },
  component: EmailsPage,
});

type Email = Schemas["email"];

function EmailsPage() {
  return (
    <div className="flex flex-col gap-6">
      <PageHeader
        title="Emails"
        description="Addresses on your account. Commits made with any of them are attributed to you."
      />
      <Region name="emails" rows={4}>
        <EmailList />
      </Region>
      <AddEmail />
      <Region name="email preferences" rows={3}>
        <Preferences />
      </Region>
    </div>
  );
}

function EmailList() {
  // A new address verifies itself in the mock a few seconds later; poll until none is pending.
  const { data: emails } = useSuspenseQuery({
    ...emailsQuery(),
    refetchInterval: (q) => (q.state.data?.some((e) => !e.verified) ? 3000 : false),
  });
  const key = emailsQuery().queryKey;

  const remove = useOptimistic<Email[], string, void>({
    queryKey: key,
    what: "Removing the address",
    mutationFn: (email) => call(api.DELETE("/user/emails", { body: { emails: [email] } })),
    apply: (old, email) => old.filter((e) => e.email !== email),
    invalidate: [userQuery().queryKey],
    onSuccess: (_r, email) => toast.success(`Removed ${email}`),
  });
  const primary = useOptimistic<Email[], string, Email[]>({
    queryKey: key,
    what: "Changing the primary address",
    mutationFn: (email) => call(ui.PATCH("/settings/emails/primary", { body: { email } })),
    apply: (old, email) => old.map((e) => ({ ...e, primary: e.email === email })),
    invalidate: [emailVisibilityQuery().queryKey, userQuery().queryKey],
    onSuccess: (_r, email) => toast.success(`${email} is now your primary address`),
  });
  const resend = useMutation({
    mutationFn: (email: string) => call(ui.POST("/settings/emails/verify", { body: { email } })),
    onError: (e) => failed("Sending the verification email", e),
    onSuccess: (r) => toast.success(`Sent a verification link to ${r.email}`),
  });

  return (
    <Panel flush>
      <ul className="divide-y" data-testid="email-list">
        {emails.map((e) => (
          <li
            key={e.email}
            className="flex flex-wrap items-center gap-2 px-4 py-2.5 text-sm"
            data-email={e.email}
          >
            <HugeiconsIcon
              icon={Mail01Icon}
              strokeWidth={2}
              className="text-muted-foreground size-4"
            />
            <span className="font-medium">{e.email}</span>
            {e.primary && <Badge variant="primary-light">Primary</Badge>}
            {e.verified ? (
              <Badge variant="success-light">Verified</Badge>
            ) : (
              <Badge variant="warning-light">Unverified</Badge>
            )}
            {e.visibility === "public" && <Badge variant="outline">Public</Badge>}
            {e.visibility === "private" && <Badge variant="outline">Private</Badge>}
            <div className="ml-auto flex items-center gap-1">
              {!e.verified && (
                <Button
                  size="xs"
                  variant="ghost"
                  onClick={() => resend.mutate(e.email)}
                  disabled={resend.isPending}
                >
                  <HugeiconsIcon icon={SentIcon} strokeWidth={2} data-icon="inline-start" />
                  Resend verification
                </Button>
              )}
              {!e.primary && e.verified && (
                <Button
                  size="xs"
                  variant="outline"
                  onClick={() => primary.mutate(e.email)}
                  disabled={primary.isPending}
                >
                  Make primary
                </Button>
              )}
              {!e.primary && (
                <Confirm
                  trigger={
                    <Button size="icon-xs" variant="ghost" aria-label={`Remove ${e.email}`}>
                      <HugeiconsIcon icon={Delete02Icon} strokeWidth={2} />
                    </Button>
                  }
                  title={`Remove ${e.email}?`}
                  description="Commits made with this address will no longer be attributed to you."
                  action="Remove"
                  onConfirm={() => remove.mutate(e.email)}
                />
              )}
            </div>
          </li>
        ))}
      </ul>
    </Panel>
  );
}

const EMAIL = /^[^\s@]+@[^\s@]+\.[^\s@]+$/;

function AddEmail() {
  const queryClient = useQueryClient();
  const add = useMutation({
    mutationFn: (email: string) => call(api.POST("/user/emails", { body: { emails: [email] } })),
    onSuccess: (created) => {
      queryClient.setQueryData<Email[]>(emailsQuery().queryKey, (old) => [
        ...(old ?? []),
        ...created,
      ]);
      void queryClient.invalidateQueries({ queryKey: emailsQuery().queryKey });
      toast.success(`Sent a verification link to ${created[0]?.email}`);
    },
  });
  const form = useForm({
    defaultValues: { email: "" },
    onSubmit: async ({ value, formApi }) => {
      try {
        await add.mutateAsync(value.email.trim());
        formApi.reset();
      } catch (error) {
        if (!applyServerErrors(form, error, { email: "email" }))
          failed("Adding the address", error);
      }
    },
  });
  return (
    <Section title="Add email address">
      <form
        noValidate
        className="flex max-w-lg items-start gap-2"
        onSubmit={(e) => {
          e.preventDefault();
          void form.handleSubmit();
        }}
        data-testid="add-email"
      >
        <div className="flex-1">
          <form.Field
            name="email"
            validators={{
              onSubmit: ({ value }) =>
                !EMAIL.test(value.trim())
                  ? "Enter an email address such as you@example.com"
                  : undefined,
            }}
          >
            {(field) => (
              <TextField
                field={field}
                label="Email address"
                type="email"
                placeholder="you@example.com"
              />
            )}
          </form.Field>
        </div>
        <form.Subscribe selector={(s) => s.isSubmitting}>
          {(submitting) => (
            <SubmitButton pending={submitting} className="mt-6">
              Add
            </SubmitButton>
          )}
        </form.Subscribe>
      </form>
    </Section>
  );
}

function Preferences() {
  const { data: prefs } = useSuspenseQuery(emailVisibilityQuery());
  const save = useOptimistic<EmailVisibility, Partial<EmailVisibility>, EmailVisibility>({
    queryKey: emailVisibilityQuery().queryKey,
    what: "Saving the email preference",
    mutationFn: (body) => call(ui.PATCH("/settings/emails/visibility", { body })),
    apply: (old, patch) => ({ ...old, ...patch }),
    invalidate: [emailsQuery().queryKey, userQuery().queryKey],
    onSuccess: () => toast.success("Email preferences saved"),
  });
  return (
    <Section title="Primary email preferences">
      <div className="flex max-w-xl flex-col gap-4" data-testid="email-preferences">
        <Field orientation="horizontal">
          <FieldContent>
            <FieldLabel htmlFor="primary-visibility">Primary email visibility</FieldLabel>
            <FieldDescription className="text-xs">
              {prefs.visibility === "public"
                ? "Your primary address can appear on your profile and in API responses."
                : "Your primary address is hidden from your profile and the API."}
            </FieldDescription>
          </FieldContent>
          <NativeSelect
            id="primary-visibility"
            size="sm"
            value={prefs.visibility}
            onChange={(e) =>
              save.mutate({ visibility: e.target.value as EmailVisibility["visibility"] })
            }
          >
            <NativeSelectOption value="public">Public</NativeSelectOption>
            <NativeSelectOption value="private">Private</NativeSelectOption>
          </NativeSelect>
        </Field>
        <Field orientation="horizontal">
          <FieldContent>
            <FieldLabel htmlFor="block-push">
              Block command line pushes that expose my email
            </FieldLabel>
            <FieldDescription className="text-xs">
              A push whose commits are authored with a private address is rejected.
            </FieldDescription>
          </FieldContent>
          <Switch
            id="block-push"
            checked={prefs.block_push}
            onCheckedChange={(on) => save.mutate({ block_push: on })}
            aria-label="Block command line pushes that expose my email"
          />
        </Field>
      </div>
    </Section>
  );
}
