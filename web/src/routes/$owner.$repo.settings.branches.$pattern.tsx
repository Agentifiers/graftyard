import { createFileRoute, Link } from "@tanstack/react-router";
import { ProtectionEditor } from "@/areas/repo-admin/protection";
import {
  branchesQuery,
  checkNamesQuery,
  protectionRulesQuery,
  usersQuery,
} from "@/areas/repo-admin/queries";
import { PageHeader, Panel } from "@/components/panel";
import { Region } from "@/components/region";

// One branch protection rule. `new` opens an empty rule.
export const Route = createFileRoute("/$owner/$repo/settings/branches/$pattern")({
  loader: ({ context: { queryClient }, params }) => {
    void queryClient.prefetchQuery(protectionRulesQuery(params));
    void queryClient.prefetchQuery(branchesQuery(params));
    void queryClient.prefetchQuery(checkNamesQuery(params));
    void queryClient.prefetchQuery(usersQuery());
  },
  component: ProtectionRulePage,
});

function ProtectionRulePage() {
  const { owner, repo, pattern } = Route.useParams();
  const isNew = pattern === "new";
  return (
    <div className="flex flex-col gap-4">
      <PageHeader
        title={
          <>
            <Link
              to="/$owner/$repo/settings/branches"
              params={{ owner, repo }}
              className="text-muted-foreground hover:underline"
            >
              Branch protection rules
            </Link>
            <span className="text-muted-foreground px-1">/</span>
            {isNew ? "New rule" : <code className="font-mono">{pattern}</code>}
          </>
        }
      />
      <Panel title={isNew ? "Protect matching branches" : "Edit rule"} flush>
        <Region name="branch protection rule" rows={8}>
          <ProtectionEditor key={pattern} params={{ owner, repo }} pattern={pattern} />
        </Region>
      </Panel>
    </div>
  );
}
