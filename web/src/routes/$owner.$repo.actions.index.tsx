import { createFileRoute } from "@tanstack/react-router";
import { ActionsShell, RunsPanel, validateRunSearch, type RunSearch } from "@/areas/actions/runs";
import { runsQuery, workflowsQuery } from "@/areas/actions/queries";
import { PageHeader } from "@/components/panel";

export const Route = createFileRoute("/$owner/$repo/actions/")({
  validateSearch: validateRunSearch,
  loaderDeps: ({ search }) => search,
  loader: ({ context, params, deps }) => {
    void context.queryClient.prefetchQuery(workflowsQuery(params));
    void context.queryClient.prefetchQuery(runsQuery({ ...params, ...deps }));
  },
  component: ActionsPage,
});

function ActionsPage() {
  const params = Route.useParams();
  const search = Route.useSearch();
  const navigate = Route.useNavigate();
  const setSearch = (patch: Partial<RunSearch>) =>
    navigate({ search: (prev) => ({ ...prev, page: undefined, ...patch }), replace: true });
  return (
    <ActionsShell {...params}>
      <PageHeader title="All workflows" description="Showing runs from all workflows" />
      <RunsPanel search={search} setSearch={setSearch} {...params} />
    </ActionsShell>
  );
}
