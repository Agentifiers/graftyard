import { createFileRoute } from "@tanstack/react-router";
import { repoQuery } from "@/areas/core/queries";
import {
  AboutForm,
  DangerZone,
  DefaultBranch,
  Features,
  LfsSetting,
  PullRequestSettings,
  RenameForm,
  TopicsEditor,
} from "@/areas/repo-admin/general";
import { ArchivedNotice } from "@/areas/repo-admin/kit";
import { adminSettingsQuery, branchesQuery, topicsQuery } from "@/areas/repo-admin/queries";
import { PageHeader, Panel } from "@/components/panel";
import { Region } from "@/components/region";

export const Route = createFileRoute("/$owner/$repo/settings/general")({
  loader: ({ context: { queryClient }, params }) => {
    for (const q of [
      repoQuery(params),
      topicsQuery(params),
      branchesQuery(params),
      adminSettingsQuery(params),
    ]) {
      void queryClient.prefetchQuery(q as never);
    }
  },
  component: GeneralPage,
});

function GeneralPage() {
  const params = Route.useParams();
  return (
    <div className="flex flex-col gap-6" data-testid="settings-general">
      <PageHeader title="General" />
      <Region name="repository settings" rows={2} fallback={null}>
        <ArchivedNotice params={params} />
      </Region>
      <Panel title="Repository">
        <Region name="repository settings" rows={4}>
          <div className="flex flex-col gap-5">
            <RenameForm key={`${params.owner}/${params.repo}`} params={params} />
            <AboutForm key={`about:${params.owner}/${params.repo}`} params={params} />
          </div>
        </Region>
      </Panel>
      <Panel title="Topics">
        <Region name="topics" rows={2}>
          <TopicsEditor key={`${params.owner}/${params.repo}`} params={params} />
        </Region>
      </Panel>
      <Panel title="Features" flush>
        <Region name="features" rows={4}>
          <Features params={params} />
        </Region>
      </Panel>
      <Panel title="Default branch">
        <Region name="branches" rows={1}>
          <DefaultBranch key={`${params.owner}/${params.repo}`} params={params} />
        </Region>
      </Panel>
      <Panel title="Pull requests" flush>
        <Region name="pull request settings" rows={5}>
          <PullRequestSettings params={params} />
        </Region>
      </Panel>
      <Panel title="Archives" flush>
        <Region name="archive settings" rows={1}>
          <LfsSetting params={params} />
        </Region>
      </Panel>
      <section className="flex flex-col gap-2">
        <h2 className="text-base font-semibold">Danger zone</h2>
        <Region name="danger zone" rows={4}>
          <DangerZone key={`${params.owner}/${params.repo}`} params={params} />
        </Region>
      </section>
    </div>
  );
}
