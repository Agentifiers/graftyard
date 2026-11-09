import { useEffect, useState } from "react";
import { createFileRoute, Link } from "@tanstack/react-router";
import { useMutation } from "@tanstack/react-query";
import { HugeiconsIcon } from "@hugeicons/react";
import {
  CheckmarkCircle02Icon,
  CommandLineIcon,
  Cancel01Icon,
  SmartPhone01Icon,
} from "@hugeicons/core-free-icons";
import { REGEXP_ONLY_DIGITS_AND_CHARS } from "input-otp";
import { AuthCard } from "@/areas/account/auth-shell";
import { failed } from "@/areas/account/kit";
import { scopeWords } from "@/areas/account/permissions";
import { call, ValidationError } from "@/areas/account/queries";
import { ui, type DeviceRequest } from "@/areas/account/ui-api";
import { Alert, AlertDescription, AlertTitle } from "@/components/reui/alert";
import { Badge } from "@/components/reui/badge";
import { Button } from "@/components/ui/button";
import {
  InputOTP,
  InputOTPGroup,
  InputOTPSeparator,
  InputOTPSlot,
} from "@/components/ui/input-otp";

// The device flow's browser half: a client such as gh shows a user code, the person types
// it here, confirms the app and its scopes, and authorizes. A code lives 900 s and works
// once (`design/DESIGN.md`, journeys; `design/SHARED.md`, OAuth).
type Step =
  | { kind: "code" }
  | { kind: "confirm"; request: DeviceRequest }
  | { kind: "done"; request: DeviceRequest; status: "authorized" | "denied" };

export const Route = createFileRoute("/login/device")({
  validateSearch: (s: Record<string, unknown>): { user_code?: string } => ({
    user_code: typeof s.user_code === "string" ? s.user_code : undefined,
  }),
  component: DevicePage,
});

const raw = (code: string) =>
  code
    .toUpperCase()
    .replace(/[^A-Z0-9]/g, "")
    .slice(0, 8);
const pretty = (code: string) => (code.length > 4 ? `${code.slice(0, 4)}-${code.slice(4)}` : code);

function DevicePage() {
  const [step, setStep] = useState<Step>({ kind: "code" });
  if (step.kind === "confirm") {
    return (
      <Confirm
        request={step.request}
        onDone={(status) => setStep({ kind: "done", request: step.request, status })}
        onBack={() => setStep({ kind: "code" })}
      />
    );
  }
  if (step.kind === "done") {
    return step.status === "authorized" ? (
      <AuthCard
        icon={CheckmarkCircle02Icon}
        title="Congratulations, you're all set!"
        subtitle={`${step.request.app.name} can now act as you.`}
      >
        <div className="flex flex-col gap-3 text-sm" data-testid="device-done">
          <p>Your device is now connected. Go back to it; it signs in on its next poll.</p>
          <p className="text-muted-foreground text-xs">
            Review or revoke it on{" "}
            <Link to="/settings/applications" className="text-primary hover:underline">
              Applications
            </Link>
            .
          </p>
        </div>
      </AuthCard>
    ) : (
      <AuthCard
        icon={Cancel01Icon}
        title="Authorization cancelled"
        subtitle={`${step.request.app.name} was not given access.`}
      >
        <p className="text-sm" data-testid="device-denied">
          The code is spent. Start again on your device if you change your mind.
        </p>
      </AuthCard>
    );
  }
  return <EnterCode onFound={(request) => setStep({ kind: "confirm", request })} />;
}

function EnterCode({ onFound }: { onFound: (r: DeviceRequest) => void }) {
  const { user_code } = Route.useSearch();
  const [code, setCode] = useState(raw(user_code ?? ""));
  const [error, setError] = useState<string>();
  const [issued, setIssued] = useState<DeviceRequest>();
  const lookup = useMutation({
    mutationFn: (c: string) => call(ui.POST("/login/device", { body: { user_code: pretty(c) } })),
    onSuccess: onFound,
    onError: (e) =>
      e instanceof ValidationError ? setError(e.message) : failed("Checking the code", e),
  });
  // Stands in for `gh auth login` asking the bare host for a code.
  const simulate = useMutation({
    mutationFn: () =>
      call(
        ui.POST("/login/device/codes", {
          body: { client_id: "178c6fc778ccc68e1d6a", scope: "repo read:org gist" },
        }),
      ),
    onSuccess: setIssued,
    onError: (e) => failed("Requesting a device code", e),
  });
  const submit = (c: string) => {
    setError(undefined);
    if (c.length === 8) lookup.mutate(c);
  };
  return (
    <AuthCard
      icon={SmartPhone01Icon}
      title="Device activation"
      subtitle="Enter the code displayed on your device"
      footer={
        <div className="flex flex-col gap-2" data-testid="device-simulator">
          <span className="flex items-center justify-center gap-1 text-xs">
            <HugeiconsIcon icon={CommandLineIcon} strokeWidth={2} className="size-3.5" />
            Mock: no device? Ask for a code as gh would.
          </span>
          <Button
            size="sm"
            variant="outline"
            onClick={() => simulate.mutate()}
            disabled={simulate.isPending}
          >
            Run gh auth login
          </Button>
          {issued && (
            <p className="font-mono text-xs" data-testid="issued-code">
              ! First copy your one-time code:{" "}
              <span className="text-foreground font-semibold">{issued.user_code}</span>
            </p>
          )}
          <p className="text-xs">Seeded: KQRT-8XPL has expired, HVNM-4ZCB is already used.</p>
        </div>
      }
    >
      <form
        noValidate
        className="flex flex-col items-center gap-3"
        onSubmit={(e) => {
          e.preventDefault();
          submit(code);
        }}
        data-testid="device-form"
      >
        <InputOTP
          maxLength={8}
          pattern={REGEXP_ONLY_DIGITS_AND_CHARS}
          value={code}
          autoFocus
          aria-label="User code"
          pasteTransformer={raw}
          onChange={(v) => {
            setCode(raw(v));
            setError(undefined);
          }}
          aria-invalid={error ? true : undefined}
        >
          <InputOTPGroup>
            {[0, 1, 2, 3].map((i) => (
              <InputOTPSlot
                key={i}
                index={i}
                className="size-9 font-mono uppercase"
                aria-invalid={error ? true : undefined}
              />
            ))}
          </InputOTPGroup>
          <InputOTPSeparator />
          <InputOTPGroup>
            {[4, 5, 6, 7].map((i) => (
              <InputOTPSlot
                key={i}
                index={i}
                className="size-9 font-mono uppercase"
                aria-invalid={error ? true : undefined}
              />
            ))}
          </InputOTPGroup>
        </InputOTP>
        {error && (
          <Alert variant="destructive" data-testid="device-error">
            <AlertTitle>{error}</AlertTitle>
          </Alert>
        )}
        <Button type="submit" className="w-full" disabled={code.length !== 8 || lookup.isPending}>
          {lookup.isPending ? "Checking…" : "Continue"}
        </Button>
      </form>
    </AuthCard>
  );
}

function useCountdown(until: string) {
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(t);
  }, []);
  return Math.max(0, Math.floor((new Date(until).getTime() - now) / 1000));
}

function Confirm({
  request,
  onDone,
  onBack,
}: {
  request: DeviceRequest;
  onDone: (s: "authorized" | "denied") => void;
  onBack: () => void;
}) {
  const left = useCountdown(request.expires_at);
  const [error, setError] = useState<string>();
  const decide = useMutation({
    mutationFn: (decision: "authorize" | "deny") =>
      call(
        ui.POST("/login/device/authorize", { body: { user_code: request.user_code, decision } }),
      ),
    onSuccess: (r) => onDone(r.status),
    onError: (e) =>
      e instanceof ValidationError ? setError(e.message) : failed("Authorizing the device", e),
  });
  return (
    <AuthCard
      icon={SmartPhone01Icon}
      title={`Authorize ${request.app.name}`}
      subtitle={
        <>
          by <span className="font-medium">{request.app.owner}</span> · code{" "}
          <code className="text-foreground font-mono font-semibold">{request.user_code}</code>
        </>
      }
    >
      <div className="flex flex-col gap-3 text-sm" data-testid="device-confirm">
        <p className="font-medium">{request.app.name} wants to:</p>
        <ul className="grid grid-cols-[auto_1fr] gap-x-2 gap-y-1.5">
          {request.scopes.map((s) => (
            <li key={s} className="col-span-2 grid grid-cols-subgrid items-start">
              <Badge variant="outline" size="sm" className="mt-0.5 justify-self-start font-mono">
                {s}
              </Badge>
              <span>{scopeWords(s)}</span>
            </li>
          ))}
          {request.scopes.length === 0 && <li>Read your public information</li>}
        </ul>
        <p className="text-muted-foreground text-xs" data-testid="device-expiry">
          {left > 0
            ? `The code expires in ${Math.floor(left / 60)}:${String(left % 60).padStart(2, "0")}.`
            : "The code has expired."}
        </p>
        {error && (
          <Alert variant="destructive" data-testid="device-authorize-error">
            <AlertTitle>{error}</AlertTitle>
            <AlertDescription>
              <button type="button" className="text-primary hover:underline" onClick={onBack}>
                Enter another code
              </button>
            </AlertDescription>
          </Alert>
        )}
        <div className="flex gap-2">
          <Button
            variant="outline"
            className="flex-1"
            onClick={() => decide.mutate("deny")}
            disabled={decide.isPending}
          >
            Cancel
          </Button>
          <Button
            className="flex-1"
            onClick={() => decide.mutate("authorize")}
            disabled={decide.isPending || left === 0}
          >
            Authorize {request.app.name}
          </Button>
        </div>
      </div>
    </AuthCard>
  );
}
