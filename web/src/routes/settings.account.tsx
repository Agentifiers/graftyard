import { useState } from "react";
import { createFileRoute, Link } from "@tanstack/react-router";
import { useMutation, useQueryClient, useSuspenseQuery } from "@tanstack/react-query";
import { useForm } from "@tanstack/react-form";
import { toast } from "sonner";
import { HugeiconsIcon } from "@hugeicons/react";
import { Alert02Icon, Delete02Icon } from "@hugeicons/core-free-icons";
import type { Schemas } from "@/api/client";
import { userQuery } from "@/areas/core/queries";
import { applyServerErrors, failed, Section, SubmitButton, TextField } from "@/areas/account/kit";
import { call } from "@/areas/account/queries";
import { ui } from "@/areas/account/ui-api";
import { PageHeader } from "@/components/panel";
import { Region } from "@/components/region";
import { Alert, AlertDescription, AlertTitle } from "@/components/reui/alert";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from "@/components/ui/dialog";

export const Route = createFileRoute("/settings/account")({
  loader: ({ context: { queryClient } }) => void queryClient.prefetchQuery(userQuery()),
  component: AccountPage,
});

function AccountPage() {
  return (
    <div className="flex flex-col gap-8">
      <PageHeader title="Account" description="Your username, and what happens to your account." />
      <Region name="account" rows={6}>
        <Rename />
        <DangerZone />
      </Region>
    </div>
  );
}

const LOGIN = /^[a-z\d](?:[a-z\d]|-(?=[a-z\d])){0,38}$/i;

function Rename() {
  const queryClient = useQueryClient();
  const { data: user } = useSuspenseQuery(userQuery());
  const rename = useMutation({
    mutationFn: (login: string) => call(ui.PATCH("/settings/account/login", { body: { login } })),
    onSuccess: (u) => {
      queryClient.setQueryData<Schemas["private-user"]>(userQuery().queryKey, u);
      toast.success(`Your username is now ${u.login}`);
    },
  });
  const form = useForm({
    defaultValues: { login: user.login },
    onSubmit: async ({ value, formApi }) => {
      try {
        await rename.mutateAsync(value.login.trim());
        formApi.reset({ login: value.login.trim() });
      } catch (error) {
        if (!applyServerErrors(form, error, { login: "login" }))
          failed("Changing the username", error);
      }
    },
  });
  return (
    <Section
      title="Change username"
      description="Links to your old profile and repositories redirect for a while, but the old name is free for anyone to claim."
    >
      <form
        noValidate
        className="flex max-w-md flex-col gap-3"
        onSubmit={(e) => {
          e.preventDefault();
          void form.handleSubmit();
        }}
        data-testid="rename-form"
      >
        <form.Field
          name="login"
          validators={{
            onChange: ({ value }) =>
              !value.trim()
                ? "Username can't be blank"
                : !LOGIN.test(value.trim())
                  ? "Use letters, digits, and single hyphens; no hyphen at the start or end; at most 39 characters"
                  : undefined,
          }}
        >
          {(field) => (
            <TextField field={field} label="Username" autoComplete="username" spellCheck={false} />
          )}
        </form.Field>
        <Alert variant="warning">
          <HugeiconsIcon icon={Alert02Icon} strokeWidth={2} />
          <AlertTitle>Renaming has side effects</AlertTitle>
          <AlertDescription>
            Git remotes that use the old name keep working through the redirect only until someone
            takes the name. Update them.
          </AlertDescription>
        </Alert>
        <form.Subscribe
          selector={(s) => [s.isSubmitting, s.values.login.trim() === user.login] as const}
        >
          {([submitting, same]) => (
            <SubmitButton pending={submitting} disabled={submitting || same} className="self-start">
              Change username
            </SubmitButton>
          )}
        </form.Subscribe>
      </form>
    </Section>
  );
}

const PHRASE = "delete my account";

function DangerZone() {
  const { data: user } = useSuspenseQuery(userQuery());
  const [open, setOpen] = useState(false);
  const [deletedAt, setDeletedAt] = useState<string>();
  const remove = useMutation({
    mutationFn: (v: { confirm_login: string; confirm_phrase: string }) =>
      call(ui.DELETE("/settings/account", { body: v })),
    onSuccess: (r) => {
      setDeletedAt(r.deleted_at);
      setOpen(false);
    },
  });
  const form = useForm({
    defaultValues: { confirm_login: "", confirm_phrase: "" },
    onSubmit: async ({ value }) => {
      try {
        await remove.mutateAsync(value);
      } catch (error) {
        if (
          !applyServerErrors(form, error, {
            confirm_login: "confirm_login",
            confirm_phrase: "confirm_phrase",
          })
        ) {
          failed("Deleting the account", error);
        }
      }
    },
  });

  if (deletedAt) {
    return (
      <Alert variant="destructive" className="mt-8" data-testid="account-deleted">
        <HugeiconsIcon icon={Delete02Icon} strokeWidth={2} />
        <AlertTitle>Your account is scheduled for deletion</AlertTitle>
        <AlertDescription>
          The mock recorded the request at {new Date(deletedAt).toLocaleString()}. Reload the page
          to start over with the seeded account.
        </AlertDescription>
      </Alert>
    );
  }

  return (
    <div className="mt-8">
      <Section title="Delete account">
        <div className="border-destructive/40 flex flex-wrap items-center gap-3 border p-3">
          <p className="text-sm flex-1">
            Deleting your account removes your repositories, forks, and stars, and frees your
            username. You own or co-own the{" "}
            <Link
              to="/orgs/$org"
              params={{ org: "graftyard" }}
              className="text-primary hover:underline"
            >
              graftyard
            </Link>{" "}
            organization, so transfer it or add another owner first.
          </p>
          <Dialog open={open} onOpenChange={setOpen}>
            <DialogTrigger
              render={
                <Button variant="destructive" size="sm">
                  Delete your account
                </Button>
              }
            />
            <DialogContent className="sm:max-w-md">
              <DialogHeader>
                <DialogTitle>Are you sure you want to do this?</DialogTitle>
                <DialogDescription>This cannot be undone.</DialogDescription>
              </DialogHeader>
              <form
                noValidate
                className="flex flex-col gap-3"
                onSubmit={(e) => {
                  e.preventDefault();
                  void form.handleSubmit();
                }}
                data-testid="delete-account-form"
              >
                <form.Field
                  name="confirm_login"
                  validators={{
                    onSubmit: ({ value }) =>
                      value === user.login ? undefined : "Type your username exactly",
                  }}
                >
                  {(field) => (
                    <TextField
                      field={field}
                      label={
                        <>
                          Your username, <code>{user.login}</code>
                        </>
                      }
                      autoComplete="off"
                    />
                  )}
                </form.Field>
                <form.Field
                  name="confirm_phrase"
                  validators={{
                    onSubmit: ({ value }) =>
                      value === PHRASE ? undefined : `Type ${PHRASE} to confirm`,
                  }}
                >
                  {(field) => (
                    <TextField
                      field={field}
                      label={
                        <>
                          To verify, type <code>{PHRASE}</code>
                        </>
                      }
                      autoComplete="off"
                    />
                  )}
                </form.Field>
                <DialogFooter>
                  <form.Subscribe
                    selector={(s) =>
                      [
                        s.isSubmitting,
                        s.values.confirm_login === user.login && s.values.confirm_phrase === PHRASE,
                      ] as const
                    }
                  >
                    {([submitting, ready]) => (
                      <Button type="submit" variant="destructive" disabled={submitting || !ready}>
                        {submitting ? "Deleting…" : "Delete this account"}
                      </Button>
                    )}
                  </form.Subscribe>
                </DialogFooter>
              </form>
            </DialogContent>
          </Dialog>
        </div>
      </Section>
    </div>
  );
}
