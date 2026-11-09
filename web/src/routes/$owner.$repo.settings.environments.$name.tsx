import { createFileRoute, Link } from "@tanstack/react-router";
import { useSuspenseQuery } from "@tanstack/react-query";
import { HugeiconsIcon } from "@hugeicons/react";
import { ArrowLeft01Icon } from "@hugeicons/core-free-icons";
import { BranchPolicies, Secrets, Variables } from "@/areas/releases/environment-config";
import { ProtectionRules } from "@/areas/releases/environment-rules";
import {
  branchPoliciesQuery,
  collaboratorsQuery,
  environmentQuery,
  envSecretsQuery,
  envVariablesQuery,
  repoTeamsQuery,
} from "@/areas/releases/queries";
import { Ago } from "@/components/github";
import { PageHeader } from "@/components/panel";
import { Region } from "@/components/region";

export const Route = createFileRoute("/$owner/$repo/settings/environments/$name")({
  loader: ({ context, params }) => {
    const { queryClient } = context;
    const repo = { owner: params.owner, repo: params.repo };
    void queryClient.prefetchQuery(environmentQuery(params));
    void queryClient.prefetchQuery(collaboratorsQuery(repo));
    void queryClient.prefetchQuery(repoTeamsQuery(repo));
    void queryClient.prefetchQuery(branchPoliciesQuery(params));
    void queryClient.prefetchQuery(envSecretsQuery(params));
    void queryClient.prefetchQuery(envVariablesQuery(params));
  },
  component: EnvironmentPage,
});

function EnvironmentPage() {
  const params = Route.useParams();
  const repo = { owner: params.owner, repo: params.repo };
  return (
    <div className="flex flex-col gap-4">
      <Link
        to="/$owner/$repo/settings/environments"
        params={repo}
        className="text-primary flex w-fit items-center gap-1 text-sm hover:underline"
      >
        <HugeiconsIcon icon={ArrowLeft01Icon} strokeWidth={2} className="size-4" />
        Environments
      </Link>
      <PageHeader
        title={
          <>
            Environments / <span data-testid="environment-name">{params.name}</span>
          </>
        }
        description={
          <Region name="environment" fallback={null}>
            <Updated />
          </Region>
        }
      />
      <Region name="protection rules" rows={6}>
        <ProtectionRules {...params} />
      </Region>
      <Region name="branch policies" rows={3}>
        <BranchPolicies {...params} />
      </Region>
      <Region name="environment secrets" rows={3}>
        <Secrets {...params} />
      </Region>
      <Region name="environment variables" rows={3}>
        <Variables {...params} />
      </Region>
    </div>
  );
}

function Updated() {
  const { data } = useSuspenseQuery(environmentQuery(Route.useParams()));
  return (
    <>
      Last changed <Ago at={data.updated_at} />. Deployments to this environment follow these rules.
    </>
  );
}
