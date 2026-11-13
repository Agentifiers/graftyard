import { useState } from "react";
import { createFileRoute, Link } from "@tanstack/react-router";
import { useMutation } from "@tanstack/react-query";
import { useForm } from "@tanstack/react-form";
import { CheckmarkCircle02Icon, Login01Icon, SmartPhone01Icon } from "@hugeicons/core-free-icons";
import { REGEXP_ONLY_DIGITS } from "input-otp";
import { AuthCard } from "@/areas/account/auth-shell";
import { failed, TextField } from "@/areas/account/kit";
import { call, ValidationError } from "@/areas/account/queries";
import { ui } from "@/areas/account/ui-api";
import { ApiError } from "@/api/client";
import { MOCK_API } from "@/mocks/browser";
import { Alert, AlertTitle } from "@/components/reui/alert";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { InputOTP, InputOTPGroup, InputOTPSlot } from "@/components/ui/input-otp";

// Sign-in: password, then the two-factor step. The mock takes any password except
// "wrong", and any six digits except 000000; a recovery code also works for mira. A
// `return_to` inside this app is where Continue goes once signed in.
type Step =
  | { kind: "password" }
  | { kind: "two-factor"; attempt: string; login: string }
  | { kind: "done"; login: string };

export const Route = createFileRoute("/login/")({
  validateSearch: (s: Record<string, unknown>): { return_to?: string } => ({
    return_to:
      typeof s.return_to === "string" &&
      s.return_to.startsWith("/") &&
      !s.return_to.startsWith("//")
        ? s.return_to
        : undefined,
  }),
  component: LoginPage,
});

function LoginPage() {
  const { return_to } = Route.useSearch();
  const [step, setStep] = useState<Step>({ kind: "password" });
  if (step.kind === "two-factor") {
    return (
      <TwoFactorStep
        attempt={step.attempt}
        login={step.login}
        onDone={(login) => setStep({ kind: "done", login })}
        onRestart={() => setStep({ kind: "password" })}
      />
    );
  }
  if (step.kind === "done") {
    return (
      <AuthCard
        icon={CheckmarkCircle02Icon}
        title={`Signed in as ${step.login}`}
        subtitle={MOCK_API ? "The mock keeps you signed in as mira either way." : undefined}
      >
        <div className="flex flex-col gap-2" data-testid="signed-in">
          <Button render={<a href={return_to ?? "/"} />}>Continue</Button>
        </div>
      </AuthCard>
    );
  }
  return <PasswordStep onNext={setStep} />;
}

function PasswordStep({ onNext }: { onNext: (s: Step) => void }) {
  const [error, setError] = useState<string>();
  const signIn = useMutation({
    mutationFn: (v: { login: string; password: string }) =>
      call(ui.POST("/login/session", { body: v })),
  });
  const form = useForm({
    defaultValues: { login: "", password: "" },
    onSubmit: async ({ value }) => {
      setError(undefined);
      try {
        const r = await signIn.mutateAsync({ login: value.login.trim(), password: value.password });
        onNext(
          r.two_factor
            ? { kind: "two-factor", attempt: r.attempt, login: r.login }
            : { kind: "done", login: r.login },
        );
      } catch (e) {
        if (e instanceof ApiError && e.status === 401) setError(e.message);
        else failed("Signing in", e);
      }
    },
  });
  return (
    <AuthCard
      icon={Login01Icon}
      title="Sign in to Graftyard"
      footer={
        <>
          Signing in on a device or the CLI?{" "}
          <Link to="/login/device" className="text-primary hover:underline">
            Enter a code
          </Link>
        </>
      }
    >
      {error && (
        <Alert variant="destructive" className="mb-3" data-testid="login-error">
          <AlertTitle>{error}</AlertTitle>
        </Alert>
      )}
      <form
        noValidate
        className="flex flex-col gap-3"
        onSubmit={(e) => {
          e.preventDefault();
          void form.handleSubmit();
        }}
        data-testid="login-form"
      >
        <form.Field
          name="login"
          validators={{
            onSubmit: ({ value }) => (value.trim() ? undefined : "Enter your username or email"),
          }}
        >
          {(field) => (
            <TextField
              field={field}
              label="Username or email address"
              autoComplete="username"
              autoFocus
            />
          )}
        </form.Field>
        <form.Field
          name="password"
          validators={{ onSubmit: ({ value }) => (value ? undefined : "Enter your password") }}
        >
          {(field) => (
            <TextField
              field={field}
              label="Password"
              type="password"
              autoComplete="current-password"
            />
          )}
        </form.Field>
        <form.Subscribe selector={(s) => s.isSubmitting}>
          {(submitting) => (
            <Button type="submit" disabled={submitting} className="w-full">
              {submitting ? "Signing in…" : "Sign in"}
            </Button>
          )}
        </form.Subscribe>
        <p className="text-muted-foreground text-center text-xs">
          Mock: any password works except “wrong”.
        </p>
      </form>
    </AuthCard>
  );
}

function TwoFactorStep({
  attempt,
  login,
  onDone,
  onRestart,
}: {
  attempt: string;
  login: string;
  onDone: (login: string) => void;
  onRestart: () => void;
}) {
  const [code, setCode] = useState("");
  const [recovery, setRecovery] = useState(false);
  const [error, setError] = useState<string>();
  const verify = useMutation({
    mutationFn: (c: string) => call(ui.POST("/login/two-factor", { body: { attempt, code: c } })),
    onSuccess: (r) => onDone(r.login),
    onError: (e) => {
      if (e instanceof ValidationError) {
        setError(e.message);
        setCode("");
      } else failed("Verifying the code", e);
    },
  });
  const submit = (c: string) => {
    setError(undefined);
    verify.mutate(c);
  };
  return (
    <AuthCard
      icon={SmartPhone01Icon}
      title="Two-factor authentication"
      subtitle={`Signing in as ${login}`}
      footer={
        <button type="button" className="text-primary hover:underline" onClick={onRestart}>
          Use a different account
        </button>
      }
    >
      <form
        noValidate
        className="flex flex-col items-center gap-3"
        onSubmit={(e) => {
          e.preventDefault();
          submit(code);
        }}
        data-testid="two-factor-form"
      >
        {recovery ? (
          <>
            <label htmlFor="recovery" className="self-start text-sm font-medium">
              Recovery code
            </label>
            <Input
              id="recovery"
              value={code}
              onChange={(e) => setCode(e.target.value)}
              placeholder="xxxxx-xxxxx"
              autoComplete="one-time-code"
            />
          </>
        ) : (
          <>
            <label htmlFor="otp" className="self-start text-sm font-medium">
              Authentication code
            </label>
            <InputOTP
              id="otp"
              maxLength={6}
              pattern={REGEXP_ONLY_DIGITS}
              value={code}
              autoFocus
              onChange={(v) => {
                setCode(v);
                setError(undefined);
              }}
              onComplete={submit}
              aria-invalid={error ? true : undefined}
            >
              <InputOTPGroup>
                {[0, 1, 2, 3, 4, 5].map((i) => (
                  <InputOTPSlot
                    key={i}
                    index={i}
                    className="size-10 text-base"
                    aria-invalid={error ? true : undefined}
                  />
                ))}
              </InputOTPGroup>
            </InputOTP>
            <p className="text-muted-foreground text-xs">
              Open your authenticator app for the code. Mock: 000000 fails.
            </p>
          </>
        )}
        {error && (
          <p className="text-destructive text-sm" role="alert" data-testid="otp-error">
            {error}
          </p>
        )}
        <Button
          type="submit"
          className="w-full"
          disabled={verify.isPending || (!recovery && code.length !== 6) || !code}
        >
          {verify.isPending ? "Verifying…" : "Verify"}
        </Button>
        <button
          type="button"
          className="text-primary text-xs hover:underline"
          onClick={() => {
            setRecovery(!recovery);
            setCode("");
            setError(undefined);
          }}
        >
          {recovery ? "Use your authenticator app" : "Use a recovery code"}
        </button>
      </form>
    </AuthCard>
  );
}
