import { createFileRoute } from "@tanstack/react-router";
import { userOrgsQuery, userQuery, userReposQuery } from "@/areas/core/queries";
import { NewRepoForm } from "@/areas/repo-admin/create";
import { gitignoreTemplatesQuery, licensesQuery } from "@/areas/repo-admin/queries";
import { PageHeader } from "@/components/panel";
import { Region } from "@/components/region";

export const Route = createFileRoute("/new")({
  loader: ({ context: { queryClient } }) => {
    void queryClient.prefetchQuery(userQuery());
    void queryClient.prefetchQuery(userOrgsQuery());
    void queryClient.prefetchQuery(userReposQuery());
    void queryClient.prefetchQuery(gitignoreTemplatesQuery());
    void queryClient.prefetchQuery(licensesQuery());
  },
  component: NewRepoPage,
});

function NewRepoPage() {
  return (
    <main className="mx-auto max-w-3xl p-4 sm:p-6">
      <PageHeader
        title="Create a new repository"
        description="A repository holds a project's files and every revision's history."
      />
      <Region name="new repository" rows={8}>
        <NewRepoForm />
      </Region>
    </main>
  );
}
