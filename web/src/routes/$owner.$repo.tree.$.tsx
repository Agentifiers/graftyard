import { useState } from "react";
import { createFileRoute, Link } from "@tanstack/react-router";
import { useSuspenseQuery } from "@tanstack/react-query";
import { HugeiconsIcon } from "@hugeicons/react";
import { FileAddIcon, GitBranchIcon, Tag01Icon } from "@hugeicons/core-free-icons";
import { Directory } from "@/areas/code/directory";
import { FileEditor } from "@/areas/code/editor";
import { FileTree } from "@/areas/code/file-tree";
import { resolveQuery, treeCommitsQuery } from "@/areas/code/queries";
import { Breadcrumbs, RefKind, RefPicker, useSwitchRef } from "@/areas/code/refs";
import { Panel } from "@/components/panel";
import { Region } from "@/components/region";
import { Button } from "@/components/ui/button";
import { Empty, EmptyDescription, EmptyHeader, EmptyTitle } from "@/components/ui/empty";
import { Skeleton } from "@/components/ui/skeleton";

// A directory at a ref. The splat is <ref>/<path>; the ref may itself hold slashes, so
// the private resolve read splits it at the longest branch, tag, or sha prefix.

export const Route = createFileRoute("/$owner/$repo/tree/$")({
  validateSearch: (search: Record<string, unknown>): { new?: true } =>
    search.new ? { new: true } : {},
  loader: async ({ context, params }) => {
    const { owner, repo, _splat = "" } = params;
    const hit = await context.queryClient
      .ensureQueryData(resolveQuery({ owner, repo, splat: _splat }))
      .catch(() => undefined);
    if (hit)
      void context.queryClient.prefetchQuery(
        treeCommitsQuery({ owner, repo, ref: hit.ref, path: hit.path }),
      );
  },
  component: TreePage,
});

function TreePage() {
  const { owner, repo, _splat = "" } = Route.useParams();
  return (
    <Region name="this directory" fallback={<Skeleton className="h-64" />}>
      <TreeBody owner={owner} repo={repo} splat={_splat} />
    </Region>
  );
}

function TreeBody({ owner, repo, splat }: { owner: string; repo: string; splat: string }) {
  const { data: at } = useSuspenseQuery(resolveQuery({ owner, repo, splat }));
  const search = Route.useSearch();
  const navigate = Route.useNavigate();
  const [adding, setAdding] = useState(Boolean(search.new));
  const switchRef = useSwitchRef("tree", { owner, repo }, at.path);
  if (at.type === "file") {
    return (
      <p className="text-sm">
        <Link
          to="/$owner/$repo/blob/$"
          params={{ owner, repo, _splat: splat }}
          className="text-primary hover:underline"
        >
          {at.path} is a file. Open it.
        </Link>
      </p>
    );
  }
  if (at.type === null) {
    return (
      <Empty className="border py-12">
        <EmptyHeader>
          <EmptyTitle>Nothing at {at.path}</EmptyTitle>
          <EmptyDescription>
            {at.ref} has no directory at that path.{" "}
            <Link
              to="/$owner/$repo/tree/$"
              params={{ owner, repo, _splat: at.ref }}
              className="text-primary hover:underline"
            >
              Go to the root
            </Link>
          </EmptyDescription>
        </EmptyHeader>
      </Empty>
    );
  }
  const close = () => {
    setAdding(false);
    void navigate({ search: {}, replace: true });
  };
  return (
    <div className="grid items-start gap-4 lg:grid-cols-[16rem_minmax(0,1fr)]">
      <aside className="bg-card hidden max-h-[70vh] overflow-y-auto rounded-sm border p-1 lg:block">
        <Region name="file tree" rows={8}>
          <FileTree owner={owner} repo={repo} ref={at.ref} sha={at.sha} path={at.path} />
        </Region>
      </aside>
      <div className="flex min-w-0 flex-col gap-3">
        <div className="flex flex-wrap items-center gap-2">
          <RefPicker owner={owner} repo={repo} current={at.ref} onPick={switchRef} />
          <RefKind kind={at.kind} />
          <Breadcrumbs owner={owner} repo={repo} ref={at.ref} path={at.path} leaf="tree" />
          <div className="ml-auto flex items-center gap-2">
            {at.kind === "branch" ? (
              <Button size="sm" variant="outline" onClick={() => setAdding(true)} disabled={adding}>
                <HugeiconsIcon icon={FileAddIcon} strokeWidth={2} data-icon="inline-start" />
                Add file
              </Button>
            ) : (
              <span className="text-muted-foreground flex items-center gap-1 text-xs">
                <HugeiconsIcon
                  icon={at.kind === "tag" ? Tag01Icon : GitBranchIcon}
                  strokeWidth={2}
                  className="size-3.5"
                />
                Switch to a branch to add files
              </span>
            )}
          </div>
        </div>
        {adding && at.kind === "branch" ? (
          <Region name="the editor" rows={6}>
            <FileEditor owner={owner} repo={repo} branch={at.ref} dir={at.path} onClose={close} />
          </Region>
        ) : (
          <Panel flush>
            <Region name="files" rows={8}>
              <Directory owner={owner} repo={repo} ref={at.ref} path={at.path} />
            </Region>
          </Panel>
        )}
      </div>
    </div>
  );
}
