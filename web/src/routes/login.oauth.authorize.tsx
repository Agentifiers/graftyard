import { useState } from "react";
import { createFileRoute, Link } from "@tanstack/react-router";
import { useMutation, useSuspenseQuery } from "@tanstack/react-query";
import { HugeiconsIcon } from "@hugeicons/react";
import {
  Alert02Icon,
  ArrowRight01Icon,
  Clock01Icon,
  Globe02Icon,
  ShieldKeyIcon,
} from "@hugeicons/core-free-icons";
import { AuthCard } from "@/areas/account/auth-shell";
import { CopyButton, failed } from "@/areas/account/kit";
import { PERMISSIONS, scopeWords } from "@/areas/account/permissions";
import { call, consentQuery } from "@/areas/account/queries";
import { ui, type Consent, type ConsentQuery } from "@/areas/account/ui-api";
import { userQuery } from "@/areas/core/queries";
import { Region } from "@/components/region";
import { Alert, AlertDescription, AlertTitle } from "@/components/reui/alert";
import { Badge } from "@/components/reui/badge";
import { Button } from "@/components/ui/button";

// The authorization-code consent page. It names the app, its owner, the scopes in plain
// words, and the redirect host, and says the code lasts 10 minutes (`design/SHARED.md`,
// OAuth). It never leaves the app: Authorize shows the redirect it would perform.
export const Route = createFileRoute("/login/oauth/authorize")({
  validateSearch: (s: Record<string, unknown>): ConsentQuery => ({
    client_id: typeof s.client_id === "string" ? s.client_id : "",
    redirect_uri: typeof s.redirect_uri === "string" ? s.redirect_uri : undefined,
    scope: typeof s.scope === "string" ? s.scope : undefined,
    state: typeof s.state === "string" ? s.state : undefined,
  }),
  loaderDeps: ({ search }) => search,
  loader: ({ context: { queryClient }, deps }) => {
    if (deps.client_id) void queryClient.prefetchQuery(consentQuery(deps));
  },
  component: AuthorizePage,
});

const EXAMPLES: { label: string; search: ConsentQuery }[] = [
  {
    label: "GitHub CLI on localhost",
    search: {
      client_id: "178c6fc778ccc68e1d6a",
      redirect_uri: "http://127.0.0.1:43117/callback",
      scope: "repo read:org gist",
      state: "s-cli-81",
    },
  },
  {
    label: "Sprint Board (new scopes)",
    search: {
      client_id: "Ov23liT0m4sSpr1ntBd",
      redirect_uri: "https://sprint.graftyard.test/auth/callback",
      scope: "repo read:user user:email",
      state: "kb-2",
    },
  },
  {
    label: "Graftyard Review (GitHub App)",
    search: {
      client_id: "Iv23li594c0d57bf08d6",
      redirect_uri: "https://review.graftyard.dev/auth/callback",
      state: "r-1",
    },
  },
  {
    label: "Bad redirect_uri",
    search: {
      client_id: "Ov23liT0m4sSpr1ntBd",
      redirect_uri: "https://evil.example/steal",
      scope: "repo",
      state: "x",
    },
  },
];

function AuthorizePage() {
  const search = Route.useSearch();
  if (!search.client_id) {
    return (
      <AuthCard
        icon={ShieldKeyIcon}
        title="Authorize an application"
        subtitle="This page is opened by an app that wants to act as you."
      >
        <div className="flex flex-col gap-2 text-sm" data-testid="consent-examples">
          <p>Mock: open one of these requests.</p>
          {EXAMPLES.map((e) => (
            <Link
              key={e.label}
              to="/login/oauth/authorize"
              search={e.search}
              className="text-primary hover:underline"
            >
              {e.label}
            </Link>
          ))}
        </div>
      </AuthCard>
    );
  }
  return (
    <Region name="authorization" rows={6}>
      <Consent q={search} />
    </Region>
  );
}

type Decision = { redirect_to: string; expires_at: string | null; decision: "authorize" | "deny" };

function Consent({ q }: { q: ConsentQuery }) {
  const { data: answer } = useSuspenseQuery(consentQuery(q));
  if (!answer.ok) {
    return (
      <AuthCard
        icon={Alert02Icon}
        title="This authorization request is not valid"
        subtitle="Nothing was shared with the app."
      >
        <Alert variant="destructive" data-testid="consent-refused">
          <HugeiconsIcon icon={Alert02Icon} strokeWidth={2} />
          <AlertTitle>The app sent a request Graftyard cannot accept</AlertTitle>
          <AlertDescription>{answer.message}</AlertDescription>
        </Alert>
      </AuthCard>
    );
  }
  return <ConsentForm q={q} consent={answer.consent} />;
}

function ConsentForm({ q, consent }: { q: ConsentQuery; consent: Consent }) {
  const { data: viewer } = useSuspenseQuery(userQuery());
  const [decided, setDecided] = useState<Decision>();
  const decide = useMutation({
    mutationFn: (decision: "authorize" | "deny") =>
      call(
        ui.POST("/oauth/authorize", {
          body: { ...q, redirect_uri: consent.redirect_uri, decision },
        }),
      ),
    onSuccess: (r, decision) => setDecided({ ...r, decision }),
    onError: (e) => failed("Authorizing the app", e),
  });

  if (decided) {
    const url = new URL(decided.redirect_to);
    return (
      <AuthCard
        icon={ArrowRight01Icon}
        title={decided.decision === "authorize" ? "Redirecting…" : "Authorization cancelled"}
        subtitle={`To ${url.host}`}
      >
        <div className="flex flex-col gap-3 text-sm" data-testid="consent-redirect">
          <p>The mock stays here. A real install would now send your browser to:</p>
          <code
            className="bg-muted border p-2 font-mono text-xs break-all"
            data-testid="redirect-to"
          >
            {decided.redirect_to}
          </code>
          <CopyButton value={decided.redirect_to} label="Copy URL" />
          {decided.expires_at && (
            <p className="text-muted-foreground text-xs">
              The code in it works once, until {new Date(decided.expires_at).toLocaleTimeString()}.
            </p>
          )}
        </div>
      </AuthCard>
    );
  }

  const github = consent.app.kind === "github_app";
  const perms =
    github && consent.permissions ? PERMISSIONS.filter((p) => consent.permissions?.[p.key]) : [];
  return (
    <AuthCard
      icon={ShieldKeyIcon}
      title={`${consent.app.name} by ${consent.app.owner}`}
      subtitle={`wants to access your ${viewer.login} account`}
      footer={
        <span className="flex items-center justify-center gap-1 text-xs">
          <HugeiconsIcon icon={Globe02Icon} strokeWidth={2} className="size-3.5" />
          Authorizing will redirect to{" "}
          <span className="text-foreground font-semibold" data-testid="redirect-host">
            {consent.redirect_host}
          </span>
        </span>
      }
    >
      <div className="flex flex-col gap-3 text-sm" data-testid="consent">
        {consent.already_authorized && (
          <Alert variant="info">
            <AlertTitle>You authorized this app before</AlertTitle>
            <AlertDescription>Authorizing again keeps the same access.</AlertDescription>
          </Alert>
        )}
        {consent.localhost && (
          <Alert variant="warning" data-testid="localhost-warning">
            <HugeiconsIcon icon={Alert02Icon} strokeWidth={2} />
            <AlertTitle>Redirects to a program on this computer</AlertTitle>
            <AlertDescription>
              Only continue if you just started this sign-in from a command line or desktop app.
            </AlertDescription>
          </Alert>
        )}
        {github ? (
          <>
            <p className="font-medium">It can act as you with these permissions:</p>
            <ul className="flex flex-col gap-1" data-testid="consent-permissions">
              {perms.map((p) => (
                <li key={p.key} className="flex items-center gap-2">
                  <Badge
                    variant={consent.permissions?.[p.key] === "write" ? "warning-light" : "outline"}
                    size="sm"
                  >
                    {consent.permissions?.[p.key] === "write"
                      ? "read and write"
                      : consent.permissions?.[p.key]}
                  </Badge>
                  {p.label}
                </li>
              ))}
            </ul>
          </>
        ) : (
          <>
            <p className="font-medium">It wants to:</p>
            <ul
              className="grid grid-cols-[auto_1fr] gap-x-2 gap-y-1.5"
              data-testid="consent-scopes"
            >
              {consent.scopes.length === 0 && (
                <li className="col-span-2">Read your public information</li>
              )}
              {consent.scopes.map((s) => (
                <li key={s} className="col-span-2 grid grid-cols-subgrid items-start">
                  <Badge
                    variant="outline"
                    size="sm"
                    className="mt-0.5 justify-self-start font-mono"
                  >
                    {s}
                  </Badge>
                  <span>{scopeWords(s)}</span>
                </li>
              ))}
            </ul>
          </>
        )}
        <p
          className="text-muted-foreground flex items-center gap-1 text-xs"
          data-testid="code-expiry"
        >
          <HugeiconsIcon icon={Clock01Icon} strokeWidth={2} className="size-3.5" />
          The authorization code it receives expires in {Math.round(
            consent.code_expires_in / 60,
          )}{" "}
          minutes and works once.
        </p>
        <p className="text-muted-foreground text-xs">
          <a
            href={consent.app.homepage_url}
            target="_blank"
            rel="noreferrer"
            className="hover:underline"
          >
            {consent.app.homepage_url}
          </a>
        </p>
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
            disabled={decide.isPending}
          >
            Authorize {consent.app.name}
          </Button>
        </div>
      </div>
    </AuthCard>
  );
}
