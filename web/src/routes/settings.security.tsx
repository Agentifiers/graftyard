import { useMemo, useState } from "react";
import { createFileRoute } from "@tanstack/react-router";
import { useMutation, useQueryClient, useSuspenseQuery } from "@tanstack/react-query";
import { useDebouncedValue } from "@tanstack/react-pacer";
import { createColumnHelper, useTable } from "@tanstack/react-table";
import { toast } from "sonner";
import { HugeiconsIcon } from "@hugeicons/react";
import {
  CommandLineIcon,
  ComputerIcon,
  Download04Icon,
  Logout02Icon,
  Search01Icon,
  SecurityCheckIcon,
  SmartPhone01Icon,
} from "@hugeicons/core-free-icons";
import { REGEXP_ONLY_DIGITS } from "input-otp";
import { userQuery } from "@/areas/core/queries";
import {
  Confirm,
  CopyButton,
  download,
  failed,
  formatDate,
  Section,
  useOptimistic,
} from "@/areas/account/kit";
import {
  call,
  securityLogQuery,
  securityQuery,
  sessionsQuery,
  ValidationError,
} from "@/areas/account/queries";
import { ui, type Security, type SecurityEvent, type Session } from "@/areas/account/ui-api";
import { Ago } from "@/components/github";
import { PageHeader, Panel } from "@/components/panel";
import { Region } from "@/components/region";
import { Alert, AlertDescription, AlertTitle } from "@/components/reui/alert";
import { Badge } from "@/components/reui/badge";
import { DataGrid, dataGridFeatures } from "@/components/reui/data-grid/data-grid";
import { DataGridPagination } from "@/components/reui/data-grid/data-grid-pagination";
import { DataGridTable } from "@/components/reui/data-grid/data-grid-table";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  InputOTP,
  InputOTPGroup,
  InputOTPSeparator,
  InputOTPSlot,
} from "@/components/ui/input-otp";

export const Route = createFileRoute("/settings/security")({
  validateSearch: (s: Record<string, unknown>): { q?: string } => ({
    q: typeof s.q === "string" && s.q ? s.q : undefined,
  }),
  loader: ({ context: { queryClient } }) => {
    void queryClient.prefetchQuery(securityQuery());
    void queryClient.prefetchQuery(sessionsQuery());
  },
  component: SecurityPage,
});

function SecurityPage() {
  return (
    <div className="flex flex-col gap-8">
      <PageHeader
        title="Sessions and security"
        description="Two-factor authentication, where you are signed in, and what happened on your account."
      />
      <Region name="two-factor authentication" rows={4}>
        <TwoFactor />
      </Region>
      <Section title="Sessions" description="Every browser and device signed in to your account.">
        <Region name="sessions" rows={3}>
          <Sessions />
        </Region>
      </Section>
      <SecurityLog />
    </div>
  );
}

function TwoFactor() {
  const queryClient = useQueryClient();
  const { data: security } = useSuspenseQuery(securityQuery());
  const [setup, setSetup] = useState<{ secret: string; otpauth_uri: string }>();
  const [codes, setCodes] = useState<string[]>();
  const [code, setCode] = useState("");
  const [codeError, setCodeError] = useState<string>();

  const begin = useMutation({
    mutationFn: () => call(ui.POST("/settings/security/two-factor/setup")),
    onError: (e) => failed("Starting two-factor setup", e),
    onSuccess: setSetup,
  });
  const enable = useMutation({
    mutationFn: (c: string) =>
      call(ui.POST("/settings/security/two-factor", { body: { code: c } })),
    onSuccess: (r) => {
      queryClient.setQueryData<Security>(securityQuery().queryKey, r.security);
      void queryClient.invalidateQueries({ queryKey: userQuery().queryKey });
      setSetup(undefined);
      setCodes(r.recovery_codes);
      setCode("");
      toast.success("Two-factor authentication is on");
    },
    onError: (e) => {
      if (e instanceof ValidationError) setCodeError(e.message);
      else failed("Turning on two-factor authentication", e);
    },
  });
  const disable = useOptimistic<Security, void, Security>({
    queryKey: securityQuery().queryKey,
    what: "Turning off two-factor authentication",
    mutationFn: () => call(ui.DELETE("/settings/security/two-factor")),
    apply: (old) => ({ ...old, two_factor: { ...old.two_factor, enabled: false } }),
    invalidate: [userQuery().queryKey],
    onSuccess: () => toast.success("Two-factor authentication is off"),
  });
  const regenerate = useMutation({
    mutationFn: () => call(ui.POST("/settings/security/recovery-codes")),
    onError: (e) => failed("Generating recovery codes", e),
    onSuccess: (r) => {
      setCodes(r.recovery_codes);
      void queryClient.invalidateQueries({ queryKey: securityQuery().queryKey });
    },
  });

  const tfa = security.two_factor;
  return (
    <Section
      title="Two-factor authentication"
      description="A second step at sign-in, from an authenticator app."
    >
      <div className="flex flex-col gap-3" data-testid="two-factor">
        <div className="flex flex-wrap items-center gap-3 border p-3">
          <HugeiconsIcon icon={SecurityCheckIcon} strokeWidth={2} className="size-6" />
          <div className="min-w-0 flex-1 text-sm">
            <p className="flex items-center gap-2 font-medium">
              Authenticator app
              {tfa.enabled ? (
                <Badge variant="success-light">Enabled</Badge>
              ) : (
                <Badge variant="warning-light">Disabled</Badge>
              )}
            </p>
            <p className="text-muted-foreground text-xs">
              {tfa.enabled
                ? `On since ${formatDate(tfa.enabled_at)}. Recovery codes generated ${formatDate(tfa.recovery_codes_generated_at)}.`
                : "Your organization graftyard requires two-factor authentication for members."}
            </p>
          </div>
          {tfa.enabled ? (
            <div className="flex gap-1">
              <Button
                size="sm"
                variant="outline"
                onClick={() => regenerate.mutate()}
                disabled={regenerate.isPending}
              >
                Regenerate recovery codes
              </Button>
              <Confirm
                trigger={
                  <Button size="sm" variant="destructive">
                    Disable
                  </Button>
                }
                title="Turn off two-factor authentication?"
                description="Your account will be protected by your password alone, and you may lose access to graftyard, which requires two-factor authentication."
                action="Disable"
                onConfirm={() => disable.mutate()}
              />
            </div>
          ) : (
            !setup && (
              <Button size="sm" onClick={() => begin.mutate()} disabled={begin.isPending}>
                Enable two-factor authentication
              </Button>
            )
          )}
        </div>

        {setup && (
          <Panel title="Set up an authenticator app">
            <div className="flex flex-col gap-3 text-sm" data-testid="two-factor-setup">
              <p>
                Add this setup key to your authenticator app, then enter the six-digit code it
                shows.
              </p>
              <div className="flex flex-wrap items-center gap-2">
                <code
                  className="bg-muted rounded-sm border px-2 py-1 font-mono text-xs tracking-widest"
                  data-testid="totp-secret"
                >
                  {setup.secret.match(/.{1,4}/g)?.join(" ")}
                </code>
                <CopyButton value={setup.secret} label="Copy key" />
              </div>
              <p className="text-muted-foreground text-xs break-all">{setup.otpauth_uri}</p>
              <form
                noValidate
                className="flex flex-col gap-2"
                onSubmit={(e) => {
                  e.preventDefault();
                  if (code.length === 6) enable.mutate(code);
                }}
              >
                <label className="text-sm font-medium" htmlFor="totp">
                  Verify the code from the app
                </label>
                <InputOTP
                  id="totp"
                  maxLength={6}
                  pattern={REGEXP_ONLY_DIGITS}
                  value={code}
                  onChange={(v) => {
                    setCode(v);
                    setCodeError(undefined);
                  }}
                  aria-invalid={codeError ? true : undefined}
                >
                  <InputOTPGroup>
                    {[0, 1, 2].map((i) => (
                      <InputOTPSlot key={i} index={i} aria-invalid={codeError ? true : undefined} />
                    ))}
                  </InputOTPGroup>
                  <InputOTPSeparator />
                  <InputOTPGroup>
                    {[3, 4, 5].map((i) => (
                      <InputOTPSlot key={i} index={i} aria-invalid={codeError ? true : undefined} />
                    ))}
                  </InputOTPGroup>
                </InputOTP>
                {codeError && (
                  <p className="text-destructive text-xs" role="alert">
                    {codeError}
                  </p>
                )}
                <p className="text-muted-foreground text-xs">
                  In this mock any six digits work except 000000.
                </p>
                <div className="flex gap-2">
                  <Button type="submit" size="sm" disabled={code.length !== 6 || enable.isPending}>
                    {enable.isPending ? "Verifying…" : "Verify and enable"}
                  </Button>
                  <Button
                    type="button"
                    size="sm"
                    variant="ghost"
                    onClick={() => setSetup(undefined)}
                  >
                    Cancel
                  </Button>
                </div>
              </form>
            </div>
          </Panel>
        )}

        {codes && <RecoveryCodes codes={codes} onDone={() => setCodes(undefined)} />}
      </div>
    </Section>
  );
}

function RecoveryCodes({ codes, onDone }: { codes: string[]; onDone: () => void }) {
  const text = codes.join("\n");
  return (
    <Alert variant="warning" data-testid="recovery-codes">
      <AlertTitle>Save your recovery codes</AlertTitle>
      <AlertDescription className="flex flex-col gap-2">
        <span>
          Each code signs you in once if you lose your authenticator. They are shown only now; any
          older codes no longer work.
        </span>
        <ol className="bg-background text-foreground grid grid-cols-2 gap-x-6 gap-y-0.5 border p-3 font-mono text-xs sm:grid-cols-4">
          {codes.map((c) => (
            <li key={c}>{c}</li>
          ))}
        </ol>
        <div className="flex flex-wrap gap-2">
          <Button
            size="sm"
            variant="outline"
            onClick={() => download("graftyard-recovery-codes.txt", `${text}\n`, "text/plain")}
          >
            <HugeiconsIcon icon={Download04Icon} strokeWidth={2} data-icon="inline-start" />
            Download
          </Button>
          <CopyButton value={text} />
          <Button size="sm" variant="ghost" onClick={onDone}>
            I have saved them
          </Button>
        </div>
      </AlertDescription>
    </Alert>
  );
}

const DEVICE_ICON = { web: ComputerIcon, mobile: SmartPhone01Icon, cli: CommandLineIcon };

function Sessions() {
  const { data } = useSuspenseQuery(sessionsQuery());
  const revoke = useOptimistic<Session[], number, void>({
    queryKey: sessionsQuery().queryKey,
    what: "Revoking the session",
    mutationFn: (id) => call(ui.DELETE("/settings/sessions/{id}", { params: { path: { id } } })),
    apply: (old, id) => old.filter((s) => s.id !== id),
    onSuccess: () => toast.success("Session revoked. That device is signed out."),
  });
  return (
    <Panel flush>
      <ul className="divide-y" data-testid="sessions">
        {data.map((s) => (
          <li
            key={s.id}
            className="flex flex-wrap items-center gap-3 px-4 py-2.5"
            data-session={s.device}
          >
            <HugeiconsIcon
              icon={DEVICE_ICON[s.kind]}
              strokeWidth={2}
              className="text-muted-foreground size-5"
            />
            <div className="min-w-0 flex-1 text-sm">
              <p className="flex items-center gap-2 font-medium">
                {s.device}
                {s.current && <Badge variant="success-light">This session</Badge>}
              </p>
              <p className="text-muted-foreground text-xs">
                {s.location} · {s.ip} · Signed in {formatDate(s.signed_in_at)} · Last active{" "}
                <Ago at={s.last_active_at} />
              </p>
            </div>
            {!s.current && (
              <Confirm
                trigger={
                  <Button size="sm" variant="outline">
                    <HugeiconsIcon icon={Logout02Icon} strokeWidth={2} data-icon="inline-start" />
                    Revoke
                  </Button>
                }
                title={`Sign out ${s.device}?`}
                description="That browser or device has to sign in again."
                action="Revoke session"
                onConfirm={() => revoke.mutate(s.id)}
              />
            )}
          </li>
        ))}
      </ul>
    </Panel>
  );
}

const ACTIONS = [
  "",
  "user.login",
  "user.failed_login",
  "personal_access_token",
  "public_key",
  "oauth_authorization",
  "integration",
  "user.two_factor",
];

function SecurityLog() {
  const { q = "" } = Route.useSearch();
  const navigate = Route.useNavigate();
  const [text, setText] = useState(q);
  // The URL (and so the query) follows the box a moment after typing stops.
  const [debounced] = useDebouncedValue(text, { wait: 300 });
  if (debounced !== q) void navigate({ search: { q: debounced || undefined }, replace: true });

  return (
    <Section
      title="Security log"
      description="Events on your account in the last 90 days, newest first."
    >
      <div className="flex flex-wrap items-center gap-2">
        <label className="relative w-full max-w-sm">
          <HugeiconsIcon
            icon={Search01Icon}
            strokeWidth={2}
            className="text-muted-foreground absolute top-1/2 left-2.5 size-3.5 -translate-y-1/2"
          />
          <Input
            type="search"
            className="pl-8"
            placeholder="Filter, such as action:user.login or Porto"
            aria-label="Filter the security log"
            value={text}
            onChange={(e) => setText(e.target.value)}
          />
        </label>
        <div className="flex flex-wrap gap-1">
          {ACTIONS.slice(1).map((a) => (
            <Button
              key={a}
              size="xs"
              variant={text === `action:${a}` ? "secondary" : "ghost"}
              onClick={() => setText(text === `action:${a}` ? "" : `action:${a}`)}
            >
              {a}
            </Button>
          ))}
        </div>
      </div>
      <Region name="security log" rows={6}>
        <LogGrid q={q} />
      </Region>
    </Section>
  );
}

const col = createColumnHelper<typeof dataGridFeatures, SecurityEvent>();

function LogGrid({ q }: { q: string }) {
  const { data } = useSuspenseQuery(securityLogQuery(q));
  const columns = useMemo(
    () =>
      col.columns([
        col.accessor("action", {
          header: "Action",
          size: 220,
          cell: (c) => <code className="font-mono text-xs">{c.getValue()}</code>,
        }),
        col.accessor("detail", {
          header: "Detail",
          size: 340,
          cell: (c) => <span className="text-sm">{c.getValue()}</span>,
        }),
        col.accessor("location", {
          header: "Where",
          size: 200,
          cell: (c) => (
            <span className="text-muted-foreground text-xs">
              {c.getValue()} · {c.row.original.ip}
            </span>
          ),
        }),
        col.accessor("created_at", {
          header: "When",
          size: 120,
          cell: (c) => (
            <span className="text-muted-foreground text-xs">
              <Ago at={c.getValue()} />
            </span>
          ),
        }),
      ]),
    [],
  );
  const table = useTable({
    features: dataGridFeatures,
    columns,
    data,
    getRowId: (r) => String(r.id),
    initialState: { pagination: { pageIndex: 0, pageSize: 10 } },
  });
  return (
    <div className="flex flex-col gap-2" data-testid="security-log" data-count={data.length}>
      <DataGrid
        table={table}
        recordCount={data.length}
        emptyMessage="No events match."
        tableLayout={{ dense: true }}
      >
        <div className="border">
          <DataGridTable />
        </div>
        <DataGridPagination sizes={[10, 25, 50]} />
      </DataGrid>
    </div>
  );
}
