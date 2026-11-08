import { useRef } from "react";
import { createFileRoute, Link } from "@tanstack/react-router";
import { useMutation, useQueryClient, useSuspenseQuery } from "@tanstack/react-query";
import { useForm } from "@tanstack/react-form";
import { toast } from "sonner";
import { HugeiconsIcon } from "@hugeicons/react";
import { Delete02Icon, ImageUploadIcon } from "@hugeicons/core-free-icons";
import { api, type Schemas } from "@/api/client";
import { userQuery } from "@/areas/core/queries";
import {
  applyServerErrors,
  failed,
  FieldErrors,
  Section,
  SubmitButton,
  TextField,
} from "@/areas/account/kit";
import { call, emailsQuery } from "@/areas/account/queries";
import { ui } from "@/areas/account/ui-api";
import { PageHeader } from "@/components/panel";
import { Region } from "@/components/region";
import { Button } from "@/components/ui/button";
import { Field, FieldLabel } from "@/components/ui/field";
import { NativeSelect, NativeSelectOption } from "@/components/ui/native-select";

export const Route = createFileRoute("/settings/profile")({
  loader: ({ context: { queryClient } }) => {
    void queryClient.prefetchQuery(userQuery());
    void queryClient.prefetchQuery(emailsQuery());
  },
  component: ProfilePage,
});

function ProfilePage() {
  return (
    <div className="flex flex-col gap-6">
      <PageHeader
        title="Public profile"
        description="What other people see on your profile and next to your work."
      />
      <div className="grid items-start gap-6 lg:grid-cols-[minmax(0,1fr)_14rem]">
        <Region name="profile" rows={8}>
          <ProfileForm />
        </Region>
        <Region name="profile picture" rows={3}>
          <AvatarPanel />
        </Region>
      </div>
    </div>
  );
}

type User = Schemas["private-user"];

const FIELDS = {
  name: "name",
  email: "email",
  bio: "bio",
  blog: "blog",
  company: "company",
  location: "location",
  twitter_username: "twitter_username",
};

function ProfileForm() {
  const queryClient = useQueryClient();
  const { data: user } = useSuspenseQuery(userQuery());
  const { data: emails } = useSuspenseQuery(emailsQuery());
  const verified = emails.filter((e) => e.verified);

  const save = useMutation({
    mutationFn: (body: Partial<Record<keyof typeof FIELDS, string>>) =>
      call(api.PATCH("/user", { body })),
    onSuccess: (updated) => {
      queryClient.setQueryData<User>(userQuery().queryKey, updated);
      toast.success("Profile updated");
    },
  });

  const form = useForm({
    defaultValues: {
      name: user.name ?? "",
      email: user.email ?? "",
      bio: user.bio ?? "",
      blog: user.blog ?? "",
      company: user.company ?? "",
      location: user.location ?? "",
      twitter_username: user.twitter_username ?? "",
    },
    onSubmit: async ({ value, formApi }) => {
      // Optimistic: the avatar menu and every other reader of the viewer see the change at once.
      const key = userQuery().queryKey;
      const previous = queryClient.getQueryData<User>(key);
      if (previous) {
        queryClient.setQueryData<User>(key, {
          ...previous,
          ...Object.fromEntries(Object.entries(value).map(([k, v]) => [k, v || null])),
        });
      }
      try {
        await save.mutateAsync(value);
        formApi.reset(value);
      } catch (error) {
        if (previous) queryClient.setQueryData(key, previous);
        applyServerErrors(form, error, FIELDS);
        failed("Saving the profile", error);
      }
    },
  });

  return (
    <form
      noValidate
      className="flex max-w-xl flex-col gap-4"
      onSubmit={(e) => {
        e.preventDefault();
        void form.handleSubmit();
      }}
      data-testid="profile-form"
    >
      <form.Field
        name="name"
        validators={{
          onChange: ({ value }) =>
            value.length > 255 ? "Name is too long (maximum is 255 characters)" : undefined,
        }}
      >
        {(field) => (
          <TextField
            field={field}
            label="Name"
            description="Your name may appear around Graftyard where you contribute or are mentioned."
            autoComplete="name"
          />
        )}
      </form.Field>
      <form.Field name="email">
        {(field) => (
          <Field data-invalid={field.state.meta.errors.length > 0 || undefined}>
            <FieldLabel htmlFor="f-email">Public email</FieldLabel>
            <NativeSelect
              id="f-email"
              className="w-full max-w-sm"
              value={field.state.value}
              onChange={(e) => {
                if (field.state.meta.errorMap.onServer) field.setErrorMap({ onServer: undefined });
                field.handleChange(e.target.value);
              }}
            >
              <NativeSelectOption value="">Don't show my email address</NativeSelectOption>
              {verified.map((e) => (
                <NativeSelectOption key={e.email} value={e.email}>
                  {e.email}
                </NativeSelectOption>
              ))}
            </NativeSelect>
            <p className="text-muted-foreground text-xs">
              Only verified addresses appear here. Manage them on the{" "}
              <Link to="/settings/emails" className="text-primary hover:underline">
                Emails
              </Link>{" "}
              page.
            </p>
            <FieldErrors field={field} />
          </Field>
        )}
      </form.Field>
      <form.Field
        name="bio"
        validators={{
          onChange: ({ value }) =>
            value.length > 160 ? "Bio is too long (maximum is 160 characters)" : undefined,
        }}
      >
        {(field) => (
          <div className="flex flex-col gap-1">
            <TextField
              field={field}
              label="Bio"
              multiline
              placeholder="Tell us a little bit about yourself"
            />
            <span
              className="text-muted-foreground self-end text-xs tabular-nums"
              data-testid="bio-count"
            >
              {field.state.value.length}/160
            </span>
          </div>
        )}
      </form.Field>
      <form.Field
        name="blog"
        validators={{
          onSubmit: ({ value }) =>
            value && !/^https?:\/\//.test(value)
              ? "URL must start with http:// or https://"
              : undefined,
        }}
      >
        {(field) => (
          <TextField field={field} label="URL" type="url" placeholder="https://example.com" />
        )}
      </form.Field>
      <div className="grid gap-4 sm:grid-cols-2">
        <form.Field name="company">
          {(field) => (
            <TextField
              field={field}
              label="Company"
              description="Mention an organization with @name."
            />
          )}
        </form.Field>
        <form.Field name="location">
          {(field) => <TextField field={field} label="Location" />}
        </form.Field>
      </div>
      <form.Field name="twitter_username">
        {(field) => <TextField field={field} label="X (Twitter) username" placeholder="handle" />}
      </form.Field>
      <form.Subscribe selector={(s) => [s.isSubmitting, s.isDirty] as const}>
        {([submitting, dirty]) => (
          <div className="flex items-center gap-3">
            <SubmitButton pending={submitting} disabled={submitting || !dirty}>
              Update profile
            </SubmitButton>
            {!dirty && <span className="text-muted-foreground text-xs">No unsaved changes</span>}
          </div>
        )}
      </form.Subscribe>
    </form>
  );
}

const MAX_BYTES = 1_000_000;

function AvatarPanel() {
  const queryClient = useQueryClient();
  const { data: user } = useSuspenseQuery(userQuery());
  const input = useRef<HTMLInputElement>(null);
  const key = userQuery().queryKey;

  const upload = useMutation({
    mutationFn: (data_url: string) => call(ui.POST("/settings/avatar", { body: { data_url } })),
    onMutate: (data_url) => {
      const previous = queryClient.getQueryData<User>(key);
      if (previous) queryClient.setQueryData<User>(key, { ...previous, avatar_url: data_url });
      return { previous };
    },
    onError: (error, _v, ctx) => {
      if (ctx?.previous) queryClient.setQueryData(key, ctx.previous);
      failed("Uploading the picture", error);
    },
    onSuccess: (u) => {
      queryClient.setQueryData(key, u);
      toast.success("Profile picture updated");
    },
  });
  const remove = useMutation({
    mutationFn: () => call(ui.DELETE("/settings/avatar")),
    onError: (error) => failed("Removing the picture", error),
    onSuccess: (u) => queryClient.setQueryData(key, u),
  });

  const pick = (file: File | undefined) => {
    if (!file) return;
    if (!/^image\/(png|jpeg|gif|webp)$/.test(file.type)) {
      toast.error("Upload a PNG, JPEG, GIF, or WebP image");
      return;
    }
    if (file.size > MAX_BYTES) {
      toast.error("Images must be 1 MB or smaller");
      return;
    }
    const reader = new FileReader();
    reader.onload = () => upload.mutate(String(reader.result));
    reader.readAsDataURL(file);
  };

  const custom =
    user.avatar_url.startsWith("data:image/") && !user.avatar_url.startsWith("data:image/svg+xml,");
  return (
    <Section title="Profile picture">
      <div
        className="hover:border-primary flex flex-col items-center gap-3 border border-dashed p-3"
        onDragOver={(e) => e.preventDefault()}
        onDrop={(e) => {
          e.preventDefault();
          pick(e.dataTransfer.files[0]);
        }}
      >
        <img
          src={user.avatar_url}
          alt={`@${user.login}`}
          className="size-36 rounded-sm border object-cover"
          data-pending={upload.isPending || undefined}
          data-testid="avatar-preview"
        />
        <input
          ref={input}
          type="file"
          accept="image/png,image/jpeg,image/gif,image/webp"
          className="sr-only"
          aria-label="Upload a profile picture"
          onChange={(e) => {
            pick(e.target.files?.[0]);
            e.target.value = "";
          }}
        />
        <div className="flex flex-wrap justify-center gap-2">
          <Button
            size="sm"
            variant="outline"
            onClick={() => input.current?.click()}
            disabled={upload.isPending}
          >
            <HugeiconsIcon icon={ImageUploadIcon} strokeWidth={2} data-icon="inline-start" />
            {upload.isPending ? "Uploading…" : "Upload"}
          </Button>
          {custom && (
            <Button
              size="sm"
              variant="ghost"
              onClick={() => remove.mutate()}
              disabled={remove.isPending}
            >
              <HugeiconsIcon icon={Delete02Icon} strokeWidth={2} data-icon="inline-start" />
              Remove
            </Button>
          )}
        </div>
        <p className="text-muted-foreground text-center text-xs">
          Drop an image here. PNG, JPEG, GIF, or WebP, up to 1 MB.
        </p>
      </div>
    </Section>
  );
}
