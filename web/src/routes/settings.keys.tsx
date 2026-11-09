import { useEffect, useState } from "react";
import { createFileRoute } from "@tanstack/react-router";
import {
  useMutation,
  useQueryClient,
  useSuspenseQuery,
  type QueryKey,
} from "@tanstack/react-query";
import { useForm } from "@tanstack/react-form";
import { toast } from "sonner";
import { HugeiconsIcon } from "@hugeicons/react";
import { Delete02Icon, Key01Icon, PlusSignIcon } from "@hugeicons/core-free-icons";
import { api, type Schemas } from "@/api/client";
import {
  applyServerErrors,
  Confirm,
  EmptyRow,
  failed,
  formatDate,
  Section,
  SubmitButton,
  TextField,
  useOptimistic,
} from "@/areas/account/kit";
import { parseGpgKey, parseSshKey, sshFingerprint } from "@/areas/account/keys";
import { call, gpgKeysQuery, signingKeysQuery, sshKeysQuery } from "@/areas/account/queries";
import { PageHeader, Panel } from "@/components/panel";
import { Region } from "@/components/region";
import { Badge } from "@/components/reui/badge";
import { Button } from "@/components/ui/button";
import { Field, FieldLabel } from "@/components/ui/field";
import { NativeSelect, NativeSelectOption } from "@/components/ui/native-select";

type Add = "ssh" | "gpg";

export const Route = createFileRoute("/settings/keys")({
  validateSearch: (s: Record<string, unknown>): { add?: Add } => ({
    add: s.add === "ssh" || s.add === "gpg" ? s.add : undefined,
  }),
  loader: ({ context: { queryClient } }) => {
    void queryClient.prefetchQuery(sshKeysQuery());
    void queryClient.prefetchQuery(signingKeysQuery());
    void queryClient.prefetchQuery(gpgKeysQuery());
  },
  component: KeysPage,
});

function KeysPage() {
  const { add } = Route.useSearch();
  const navigate = Route.useNavigate();
  const open = (a?: Add) => void navigate({ search: { add: a }, replace: true });
  return (
    <div className="flex flex-col gap-8">
      <PageHeader
        title="SSH and GPG keys"
        description="Keys that let you push over SSH and sign your commits and tags."
      />
      <Section
        title="SSH keys"
        description="Authentication keys clone and push over SSH. Signing keys verify commits signed with SSH."
        actions={
          add !== "ssh" && (
            <Button size="sm" onClick={() => open("ssh")}>
              <HugeiconsIcon icon={PlusSignIcon} strokeWidth={2} data-icon="inline-start" />
              New SSH key
            </Button>
          )
        }
      >
        {add === "ssh" && <AddSshKey onDone={() => open()} />}
        <Region name="SSH keys" rows={3}>
          <SshKeyList kind="authentication" />
        </Region>
        <Region name="SSH signing keys" rows={2}>
          <SshKeyList kind="signing" />
        </Region>
      </Section>
      <Section
        title="GPG keys"
        description="Commits and tags signed with one of these keys show as verified."
        actions={
          add !== "gpg" && (
            <Button size="sm" onClick={() => open("gpg")}>
              <HugeiconsIcon icon={PlusSignIcon} strokeWidth={2} data-icon="inline-start" />
              New GPG key
            </Button>
          )
        }
      >
        {add === "gpg" && <AddGpgKey onDone={() => open()} />}
        <Region name="GPG keys" rows={2}>
          <GpgKeyList />
        </Region>
      </Section>
    </div>
  );
}

function Fingerprint({ keyText }: { keyText: string }) {
  const [fp, setFp] = useState("");
  useEffect(() => {
    void sshFingerprint(keyText).then(setFp);
  }, [keyText]);
  return (
    <code className="text-muted-foreground font-mono text-xs break-all" data-testid="fingerprint">
      {fp || "…"}
    </code>
  );
}

type SshRow = {
  id: number;
  title: string;
  key: string;
  created_at: string;
  last_used?: string | null;
};

function SshKeyList({ kind }: { kind: "authentication" | "signing" }) {
  const query = kind === "authentication" ? sshKeysQuery() : signingKeysQuery();
  const { data } = useSuspenseQuery<SshRow[]>({
    queryKey: query.queryKey,
    queryFn: query.queryFn as () => Promise<SshRow[]>,
  });
  const remove = useOptimistic<SshRow[], number, void>({
    queryKey: query.queryKey,
    what: "Deleting the key",
    mutationFn: (id) =>
      kind === "authentication"
        ? call(api.DELETE("/user/keys/{key_id}", { params: { path: { key_id: id } } }))
        : call(
            api.DELETE("/user/ssh_signing_keys/{ssh_signing_key_id}", {
              params: { path: { ssh_signing_key_id: id } },
            }),
          ),
    apply: (old, id) => old.filter((k) => k.id !== id),
    onSuccess: () => toast.success("Key deleted"),
  });
  return (
    <Panel title={kind === "authentication" ? "Authentication keys" : "Signing keys"} flush>
      {data.length === 0 ? (
        <EmptyRow>
          {kind === "authentication"
            ? "No SSH authentication keys. Add one to push over SSH."
            : "No SSH signing keys."}
        </EmptyRow>
      ) : (
        <ul className="divide-y" data-testid={`ssh-${kind}`}>
          {data.map((k) => {
            const parsed = parseSshKey(k.key);
            return (
              <li key={k.id} className="flex items-start gap-3 px-4 py-3" data-key-title={k.title}>
                <div className="flex w-16 shrink-0 flex-col items-center gap-1 pt-0.5">
                  <HugeiconsIcon icon={Key01Icon} strokeWidth={2} className="size-6" />
                  <Badge variant="outline" size="sm">
                    {"type" in parsed
                      ? parsed.type.replace(/^ssh-|^ecdsa-sha2-/, "").toUpperCase()
                      : "SSH"}
                  </Badge>
                </div>
                <div className="min-w-0 flex-1">
                  <p className="text-sm font-semibold">{k.title}</p>
                  <Fingerprint keyText={k.key} />
                  <p className="text-muted-foreground mt-1 text-xs">
                    Added on {formatDate(k.created_at)}
                    {kind === "authentication" &&
                      ` · ${k.last_used ? `Last used ${formatDate(k.last_used)}` : "Never used"}`}
                  </p>
                </div>
                <Confirm
                  trigger={
                    <Button size="sm" variant="destructive" aria-label={`Delete ${k.title}`}>
                      <HugeiconsIcon icon={Delete02Icon} strokeWidth={2} data-icon="inline-start" />
                      Delete
                    </Button>
                  }
                  title={`Delete ${k.title}?`}
                  description={
                    kind === "authentication"
                      ? "Anything using this key loses SSH access to your repositories."
                      : "Commits signed with this key will no longer show as verified."
                  }
                  action="Delete key"
                  onConfirm={() => remove.mutate(k.id)}
                />
              </li>
            );
          })}
        </ul>
      )}
    </Panel>
  );
}

function AddSshKey({ onDone }: { onDone: () => void }) {
  const queryClient = useQueryClient();
  const add = useMutation({
    mutationFn: async (v: { title: string; key: string; type: "authentication" | "signing" }) =>
      v.type === "authentication"
        ? call(api.POST("/user/keys", { body: { title: v.title, key: v.key } }))
        : call(api.POST("/user/ssh_signing_keys", { body: { title: v.title, key: v.key } })),
    onSuccess: (_k, v) => {
      const key: QueryKey = (v.type === "authentication" ? sshKeysQuery() : signingKeysQuery())
        .queryKey;
      void queryClient.invalidateQueries({ queryKey: key });
      toast.success(`Added ${v.title}`);
    },
  });
  const form = useForm({
    defaultValues: { title: "", type: "authentication" as "authentication" | "signing", key: "" },
    onSubmit: async ({ value }) => {
      try {
        await add.mutateAsync({ ...value, title: value.title.trim(), key: value.key.trim() });
        onDone();
      } catch (error) {
        if (!applyServerErrors(form, error, { title: "title", key: "key" }))
          failed("Adding the key", error);
      }
    },
  });
  return (
    <Panel title="Add new SSH key">
      <form
        noValidate
        className="flex flex-col gap-3"
        onSubmit={(e) => {
          e.preventDefault();
          void form.handleSubmit();
        }}
        data-testid="add-ssh-key"
      >
        <div className="grid gap-3 sm:grid-cols-[minmax(0,1fr)_12rem]">
          <form.Field
            name="title"
            validators={{
              onSubmit: ({ value }) => (value.trim() ? undefined : "Title can't be blank"),
            }}
          >
            {(field) => (
              <TextField field={field} label="Title" placeholder="work laptop" autoComplete="off" />
            )}
          </form.Field>
          <form.Field name="type">
            {(field) => (
              <Field>
                <FieldLabel htmlFor="f-type">Key type</FieldLabel>
                <NativeSelect
                  id="f-type"
                  className="w-full"
                  value={field.state.value}
                  onChange={(e) =>
                    field.handleChange(e.target.value as "authentication" | "signing")
                  }
                >
                  <NativeSelectOption value="authentication">Authentication key</NativeSelectOption>
                  <NativeSelectOption value="signing">Signing key</NativeSelectOption>
                </NativeSelect>
              </Field>
            )}
          </form.Field>
        </div>
        <form.Field
          name="key"
          validators={{
            onChange: ({ value }) => {
              if (!value.trim()) return undefined;
              const parsed = parseSshKey(value);
              return "error" in parsed ? parsed.error : undefined;
            },
            onSubmit: ({ value }) => (value.trim() ? undefined : "Key can't be blank"),
          }}
        >
          {(field) => (
            <TextField
              field={field}
              label="Key"
              multiline
              mono
              placeholder="Begins with ssh-ed25519, ssh-rsa, ecdsa-sha2-nistp256, or sk-ssh-ed25519@openssh.com"
              description={<KeyPreview text={field.state.value} />}
            />
          )}
        </form.Field>
        <div className="flex gap-2">
          <form.Subscribe selector={(s) => s.isSubmitting}>
            {(submitting) => <SubmitButton pending={submitting}>Add SSH key</SubmitButton>}
          </form.Subscribe>
          <Button type="button" size="sm" variant="ghost" onClick={onDone}>
            Cancel
          </Button>
        </div>
      </form>
    </Panel>
  );
}

function KeyPreview({ text }: { text: string }) {
  const parsed = parseSshKey(text);
  if (!text.trim() || "error" in parsed) return <>Paste the contents of your .pub file.</>;
  return (
    <>
      {parsed.type} key, fingerprint <Fingerprint keyText={text} />
    </>
  );
}

type GpgKey = Schemas["gpg-key"];

function GpgKeyList() {
  const { data } = useSuspenseQuery(gpgKeysQuery());
  const remove = useOptimistic<GpgKey[], number, void>({
    queryKey: gpgKeysQuery().queryKey,
    what: "Deleting the GPG key",
    mutationFn: (id) =>
      call(api.DELETE("/user/gpg_keys/{gpg_key_id}", { params: { path: { gpg_key_id: id } } })),
    apply: (old, id) => old.filter((k) => k.id !== id),
    onSuccess: () => toast.success("GPG key deleted"),
  });
  return (
    <Panel flush>
      {data.length === 0 ? (
        <EmptyRow>No GPG keys. Add one to show your signed commits as verified.</EmptyRow>
      ) : (
        <ul className="divide-y" data-testid="gpg-keys">
          {data.map((k) => (
            <li key={k.id} className="flex items-start gap-3 px-4 py-3" data-key-id={k.key_id}>
              <div className="flex w-16 shrink-0 flex-col items-center gap-1 pt-0.5">
                <HugeiconsIcon icon={Key01Icon} strokeWidth={2} className="size-6" />
                <Badge variant="outline" size="sm">
                  GPG
                </Badge>
              </div>
              <div className="min-w-0 flex-1 text-sm">
                <p className="font-semibold">{k.name ?? k.emails[0]?.email ?? "GPG key"}</p>
                <p className="flex flex-wrap gap-x-2 text-xs">
                  <span className="text-muted-foreground">Key ID:</span>
                  <code className="font-mono">{k.key_id}</code>
                </p>
                <p className="flex flex-wrap items-center gap-1 text-xs">
                  <span className="text-muted-foreground">Email addresses:</span>
                  {k.emails.length === 0 && <span>none</span>}
                  {k.emails.map((e) => (
                    <span key={e.email} className="flex items-center gap-1">
                      {e.email}
                      <Badge variant={e.verified ? "success-light" : "warning-light"} size="sm">
                        {e.verified ? "Verified" : "Unverified"}
                      </Badge>
                    </span>
                  ))}
                </p>
                <p className="text-xs">
                  <span className="text-muted-foreground">Subkeys:</span>{" "}
                  {k.subkeys.length === 0 ? "none" : k.subkeys.map((s) => s.key_id).join(", ")}
                </p>
                <p className="text-muted-foreground mt-1 text-xs">
                  Added on {formatDate(k.created_at)} · {k.can_sign ? "Can sign" : "Cannot sign"} ·{" "}
                  {k.expires_at ? `Expires ${formatDate(k.expires_at)}` : "Does not expire"}
                </p>
              </div>
              <Confirm
                trigger={
                  <Button size="sm" variant="destructive" aria-label={`Delete GPG key ${k.key_id}`}>
                    <HugeiconsIcon icon={Delete02Icon} strokeWidth={2} data-icon="inline-start" />
                    Delete
                  </Button>
                }
                title={`Delete GPG key ${k.key_id}?`}
                description="Commits signed with this key will show as unverified."
                action="Delete key"
                onConfirm={() => remove.mutate(k.id)}
              />
            </li>
          ))}
        </ul>
      )}
    </Panel>
  );
}

function AddGpgKey({ onDone }: { onDone: () => void }) {
  const queryClient = useQueryClient();
  const [preview, setPreview] = useState<string>();
  const add = useMutation({
    mutationFn: (v: { name: string; armored_public_key: string }) =>
      call(
        api.POST("/user/gpg_keys", {
          body: { name: v.name || undefined, armored_public_key: v.armored_public_key },
        }),
      ),
    onSuccess: (k) => {
      void queryClient.invalidateQueries({ queryKey: gpgKeysQuery().queryKey });
      toast.success(`Added GPG key ${k.key_id}`);
    },
  });
  const form = useForm({
    defaultValues: { name: "", armored_public_key: "" },
    onSubmit: async ({ value }) => {
      try {
        await add.mutateAsync({
          name: value.name.trim(),
          armored_public_key: value.armored_public_key.trim(),
        });
        onDone();
      } catch (error) {
        if (!applyServerErrors(form, error, { key: "armored_public_key", name: "name" }))
          failed("Adding the GPG key", error);
      }
    },
  });
  return (
    <Panel title="Add new GPG key">
      <form
        noValidate
        className="flex flex-col gap-3"
        onSubmit={(e) => {
          e.preventDefault();
          void form.handleSubmit();
        }}
        data-testid="add-gpg-key"
      >
        <form.Field name="name">
          {(field) => (
            <TextField field={field} label="Title" placeholder="Optional" autoComplete="off" />
          )}
        </form.Field>
        <form.Field
          name="armored_public_key"
          validators={{
            onChangeAsync: async ({ value }) => {
              if (!value.trim()) {
                setPreview(undefined);
                return undefined;
              }
              const info = await parseGpgKey(value);
              if ("error" in info) {
                setPreview(undefined);
                return info.error;
              }
              setPreview(
                `Key ID ${info.keyId.slice(-16)} for ${info.emails.join(", ") || "no email"}`,
              );
              return undefined;
            },
            onChangeAsyncDebounceMs: 250,
            onSubmit: ({ value }) => (value.trim() ? undefined : "Key can't be blank"),
          }}
        >
          {(field) => (
            <TextField
              field={field}
              label="Key"
              multiline
              mono
              placeholder="Begins with -----BEGIN PGP PUBLIC KEY BLOCK-----"
              description={preview ?? "Paste the output of gpg --armor --export <key id>."}
            />
          )}
        </form.Field>
        <div className="flex gap-2">
          <form.Subscribe selector={(s) => s.isSubmitting}>
            {(submitting) => <SubmitButton pending={submitting}>Add GPG key</SubmitButton>}
          </form.Subscribe>
          <Button type="button" size="sm" variant="ghost" onClick={onDone}>
            Cancel
          </Button>
        </div>
      </form>
    </Panel>
  );
}
