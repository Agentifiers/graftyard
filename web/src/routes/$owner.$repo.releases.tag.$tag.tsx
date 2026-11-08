import { useState } from "react";
import { createFileRoute, Link, useNavigate } from "@tanstack/react-router";
import { useMutation, useQueryClient, useSuspenseQuery } from "@tanstack/react-query";
import { HugeiconsIcon } from "@hugeicons/react";
import { ArrowLeft01Icon, Delete02Icon, PencilEdit02Icon } from "@hugeicons/core-free-icons";
import { toast } from "sonner";
import { api } from "@/api/client";
import { errorText, send } from "@/areas/releases/api";
import { latestReleaseQuery, releaseByTagQuery, releasesKey } from "@/areas/releases/queries";
import { ReleaseCard } from "@/areas/releases/release-card";
import { Region } from "@/components/region";
import {
  AlertDialog,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";

export const Route = createFileRoute("/$owner/$repo/releases/tag/$tag")({
  loader: ({ context, params }) => {
    void context.queryClient.prefetchQuery(releaseByTagQuery(params));
    void context.queryClient.prefetchQuery(latestReleaseQuery(params));
  },
  component: ReleasePage,
});

function ReleasePage() {
  const params = Route.useParams();
  return (
    <div className="flex flex-col gap-4">
      <Link
        to="/$owner/$repo/releases"
        params={{ owner: params.owner, repo: params.repo }}
        className="text-primary flex w-fit items-center gap-1 text-sm hover:underline"
      >
        <HugeiconsIcon icon={ArrowLeft01Icon} strokeWidth={2} className="size-4" />
        Releases
      </Link>
      <Region name="release" rows={8}>
        <Release />
      </Region>
    </div>
  );
}

function Release() {
  const params = Route.useParams();
  const repo = { owner: params.owner, repo: params.repo };
  const { data: release } = useSuspenseQuery(releaseByTagQuery(params));
  const { data: latest } = useSuspenseQuery(latestReleaseQuery(repo));
  const [confirming, setConfirming] = useState(false);
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const remove = useMutation({
    mutationFn: () =>
      send(
        api.DELETE("/repos/{owner}/{repo}/releases/{release_id}", {
          params: { path: { ...repo, release_id: release.id } },
        }),
      ),
    onSuccess: async () => {
      setConfirming(false);
      toast.success(`Deleted release ${release.name || release.tag_name}`);
      queryClient.removeQueries({ queryKey: releaseByTagQuery(params).queryKey });
      await queryClient.invalidateQueries({ queryKey: releasesKey(repo) });
      void navigate({ to: "/$owner/$repo/releases", params: repo });
    },
    onError: (e) => toast.error("Release not deleted", { description: errorText(e) }),
  });

  return (
    <>
      <ReleaseCard
        {...repo}
        release={release}
        latest={release.id === latest?.id}
        actions={
          <>
            <Button
              size="sm"
              variant="outline"
              render={
                <Link
                  to="/$owner/$repo/releases/edit/$tag"
                  params={{ ...repo, tag: release.tag_name }}
                />
              }
            >
              <HugeiconsIcon icon={PencilEdit02Icon} strokeWidth={2} data-icon="inline-start" />
              Edit
            </Button>
            <Button size="sm" variant="destructive" onClick={() => setConfirming(true)}>
              <HugeiconsIcon icon={Delete02Icon} strokeWidth={2} data-icon="inline-start" />
              Delete
            </Button>
          </>
        }
      />
      <AlertDialog open={confirming} onOpenChange={setConfirming}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Delete this release?</AlertDialogTitle>
            <AlertDialogDescription>
              This deletes {release.name || release.tag_name} and its {release.assets.length}{" "}
              assets. The tag <code className="font-mono">{release.tag_name}</code> stays.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <Button
              variant="destructive"
              disabled={remove.isPending}
              onClick={() => remove.mutate()}
            >
              {remove.isPending ? "Deleting…" : "Delete this release"}
            </Button>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </>
  );
}
