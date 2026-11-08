import { useForm } from "@tanstack/react-form";
import { Badge } from "@/components/reui/badge";
import { Checkbox } from "@/components/ui/checkbox";
import { FieldDescription, FieldLegend, FieldSet } from "@/components/ui/field";
import { RadioGroup, RadioGroupItem } from "@/components/ui/radio-group";
import { Switch } from "@/components/ui/switch";
import { applyServerErrors, failed, FieldErrors, SubmitButton, TextField } from "./kit";
import { APP_EVENTS, PERMISSIONS } from "./permissions";
import { PermissionMatrix } from "./token-fields";
import type { AppManifest, AppPermissions } from "./ui-api";

// The GitHub App form: the manifest fields GitHub's manifest flow takes
// (https://docs.github.com/enterprise-server@3.22/apps/sharing-github-apps/registering-a-github-app-from-a-manifest),
// used to register an App and to edit one.

export type AppFormValues = {
  name: string;
  description: string;
  homepage_url: string;
  callback_urls: string;
  request_oauth_on_install: boolean;
  setup_url: string;
  setup_on_update: boolean;
  hook_active: boolean;
  hook_url: string;
  hook_secret: string;
  permissions: AppPermissions;
  events: string[];
  public: boolean;
};

export const emptyApp: AppFormValues = {
  name: "",
  description: "",
  homepage_url: "",
  callback_urls: "",
  request_oauth_on_install: false,
  setup_url: "",
  setup_on_update: false,
  hook_active: true,
  hook_url: "",
  hook_secret: "",
  permissions: { metadata: "read" },
  events: [],
  public: false,
};

export function toManifest(v: AppFormValues): AppManifest {
  return {
    name: v.name.trim(),
    description: v.description.trim(),
    homepage_url: v.homepage_url.trim(),
    callback_urls: v.callback_urls.split(/\s+/).filter(Boolean),
    request_oauth_on_install: v.request_oauth_on_install,
    setup_url: v.setup_url.trim(),
    setup_on_update: v.setup_on_update,
    hook_attributes: { active: v.hook_active, url: v.hook_url.trim(), secret: v.hook_secret },
    permissions: v.permissions,
    // An event whose permission is gone cannot be delivered, so it drops out.
    events: v.events.filter((e) => availableEvents(v.permissions).includes(e)),
    public: v.public,
  };
}

export function availableEvents(permissions: AppPermissions) {
  return APP_EVENTS.filter((e) => permissions[e.needs]).map((e) => e.name);
}

const SERVER = {
  name: "name",
  homepage_url: "homepage_url",
  callback_urls: "callback_urls",
  setup_url: "setup_url",
  "hook_attributes.url": "hook_url",
  events: "events",
};

const url = (v: string, required = true) =>
  !v.trim()
    ? required
      ? "Enter a URL"
      : undefined
    : /^https?:\/\/\S+/.test(v.trim())
      ? undefined
      : "Enter an http or https URL";

export function AppForm({
  initial,
  submit,
  label,
  editing,
  ownerLogin,
}: {
  initial: AppFormValues;
  submit: (m: AppManifest) => Promise<unknown>;
  label: string;
  editing?: boolean;
  ownerLogin: string;
}) {
  const form = useForm({
    defaultValues: initial,
    onSubmit: async ({ value, formApi }) => {
      try {
        await submit(toManifest(value));
        formApi.reset({ ...value, hook_secret: "" });
      } catch (error) {
        if (!applyServerErrors(form, error, SERVER)) failed(label, error);
      }
    },
  });
  return (
    <form
      noValidate
      className="flex max-w-2xl flex-col gap-6"
      onSubmit={(e) => {
        e.preventDefault();
        void form.handleSubmit();
      }}
      data-testid="app-form"
    >
      <FieldSet>
        <FieldLegend>About</FieldLegend>
        <form.Field
          name="name"
          validators={{
            onChange: ({ value }) =>
              value.length > 34 ? "Name is too long (maximum is 34 characters)" : undefined,
            onSubmit: ({ value }) => (value.trim() ? undefined : "Name can't be blank"),
          }}
        >
          {(field) => (
            <TextField
              field={field}
              label="GitHub App name"
              description={`Its slug: ${
                field.state.value
                  .toLowerCase()
                  .replace(/[^a-z0-9]+/g, "-")
                  .replace(/^-|-$/g, "") || "…"
              }`}
            />
          )}
        </form.Field>
        <form.Field name="description">
          {(field) => (
            <TextField
              field={field}
              label="Description"
              multiline
              placeholder="Shown to people who install it"
            />
          )}
        </form.Field>
        <form.Field name="homepage_url" validators={{ onSubmit: ({ value }) => url(value) }}>
          {(field) => (
            <TextField
              field={field}
              label="Homepage URL"
              type="url"
              placeholder="https://example.com"
            />
          )}
        </form.Field>
      </FieldSet>

      <FieldSet>
        <FieldLegend>Identifying and authorizing users</FieldLegend>
        <form.Field
          name="callback_urls"
          validators={{
            onSubmit: ({ value }) =>
              value
                .split(/\s+/)
                .filter(Boolean)
                .map((u) => url(u))
                .find(Boolean),
          }}
        >
          {(field) => (
            <TextField
              field={field}
              label="Callback URLs"
              multiline
              placeholder="https://example.com/auth/callback"
              description="One per line. Where people return after they authorize the App to act as them."
            />
          )}
        </form.Field>
        <form.Field name="request_oauth_on_install">
          {(field) => (
            <label className="flex items-center gap-2 text-sm">
              <Checkbox checked={field.state.value} onCheckedChange={field.handleChange} />
              Request user authorization during installation
            </label>
          )}
        </form.Field>
      </FieldSet>

      <FieldSet>
        <FieldLegend>Post installation</FieldLegend>
        <form.Field name="setup_url" validators={{ onSubmit: ({ value }) => url(value, false) }}>
          {(field) => (
            <TextField
              field={field}
              label="Setup URL (optional)"
              type="url"
              description="Where people go after they install the App."
            />
          )}
        </form.Field>
        <form.Field name="setup_on_update">
          {(field) => (
            <label className="flex items-center gap-2 text-sm">
              <Checkbox checked={field.state.value} onCheckedChange={field.handleChange} />
              Redirect on update
            </label>
          )}
        </form.Field>
      </FieldSet>

      <FieldSet>
        <FieldLegend>Webhook</FieldLegend>
        <form.Field name="hook_active">
          {(field) => (
            <label className="flex items-center gap-2 text-sm">
              <Switch
                checked={field.state.value}
                onCheckedChange={field.handleChange}
                aria-label="Webhook active"
              />
              Active
              <span className="text-muted-foreground text-xs">
                Deliver events to the webhook URL.
              </span>
            </label>
          )}
        </form.Field>
        <form.Subscribe selector={(s) => s.values.hook_active}>
          {(active) =>
            active && (
              <>
                <form.Field
                  name="hook_url"
                  validators={{
                    onSubmit: ({ value }) =>
                      form.getFieldValue("hook_active") ? url(value) : undefined,
                  }}
                >
                  {(field) => (
                    <TextField
                      field={field}
                      label="Webhook URL"
                      type="url"
                      placeholder="https://example.com/webhook"
                    />
                  )}
                </form.Field>
                <form.Field name="hook_secret">
                  {(field) => (
                    <TextField
                      field={field}
                      label="Webhook secret"
                      type="password"
                      autoComplete="new-password"
                      description={
                        editing
                          ? "Leave blank to keep the current secret."
                          : "Signs each delivery's X-Hub-Signature-256 so your server can check it."
                      }
                    />
                  )}
                </form.Field>
              </>
            )
          }
        </form.Subscribe>
      </FieldSet>

      <form.Field name="permissions">
        {(field) => (
          <FieldSet>
            <FieldLegend>Permissions</FieldLegend>
            <FieldDescription className="text-xs">
              Installations get these permissions. Raising one asks every installer to approve it
              again.
            </FieldDescription>
            <PermissionMatrix
              value={field.state.value}
              onChange={field.handleChange}
              groups={["repository", "organization", "account"]}
            />
          </FieldSet>
        )}
      </form.Field>

      <form.Subscribe selector={(s) => [s.values.permissions, s.values.hook_active] as const}>
        {([permissions, active]) => (
          <form.Field name="events">
            {(field) => {
              const available = availableEvents(permissions);
              return (
                <FieldSet>
                  <FieldLegend>Subscribe to events</FieldLegend>
                  <FieldDescription className="text-xs">
                    {active
                      ? "Only events your permissions cover are offered."
                      : "Turn the webhook on to subscribe to events."}
                  </FieldDescription>
                  <div className="grid gap-1.5 sm:grid-cols-3" data-testid="app-events">
                    {APP_EVENTS.map((e) => {
                      const ok = active && available.includes(e.name);
                      return (
                        <label
                          key={e.name}
                          className="flex items-center gap-2 text-sm data-disabled:opacity-50"
                          data-disabled={!ok || undefined}
                        >
                          <Checkbox
                            disabled={!ok}
                            checked={ok && field.state.value.includes(e.name)}
                            onCheckedChange={(on) =>
                              field.handleChange(
                                on
                                  ? [...field.state.value, e.name]
                                  : field.state.value.filter((x) => x !== e.name),
                              )
                            }
                          />
                          <code className="font-mono text-xs">{e.name}</code>
                        </label>
                      );
                    })}
                  </div>
                  <FieldErrors field={field} />
                </FieldSet>
              );
            }}
          </form.Field>
        )}
      </form.Subscribe>

      <form.Field name="public">
        {(field) => (
          <FieldSet>
            <FieldLegend>Where can this GitHub App be installed?</FieldLegend>
            <RadioGroup
              value={field.state.value ? "any" : "only"}
              onValueChange={(v) => field.handleChange(v === "any")}
            >
              <label className="flex items-start gap-2 text-sm">
                <RadioGroupItem value="only" className="mt-0.5" />
                <span>
                  <span className="font-medium">Only on this account</span>
                  <span className="text-muted-foreground block text-xs">
                    Only {ownerLogin} can install it.
                  </span>
                </span>
              </label>
              <label className="flex items-start gap-2 text-sm">
                <RadioGroupItem value="any" className="mt-0.5" />
                <span>
                  <span className="font-medium">Any account</span>
                  <span className="text-muted-foreground block text-xs">
                    Any user or organization on this instance can install it.
                  </span>
                </span>
              </label>
            </RadioGroup>
          </FieldSet>
        )}
      </form.Field>

      <form.Subscribe selector={(s) => [s.isSubmitting, s.values.permissions] as const}>
        {([submitting, permissions]) => (
          <div className="flex items-center gap-3 border-t pt-3">
            <SubmitButton pending={submitting}>{label}</SubmitButton>
            <Badge variant="outline">
              {PERMISSIONS.filter((p) => permissions[p.key]).length} permissions
            </Badge>
          </div>
        )}
      </form.Subscribe>
    </form>
  );
}
