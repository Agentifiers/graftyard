import { useState } from "react";
import { createFileRoute, Link } from "@tanstack/react-router";
import { useSuspenseQuery } from "@tanstack/react-query";
import type { Schemas } from "@/api/client";
import { decodeText } from "@/areas/code/base64";
import { BlobView } from "@/areas/code/blob-view";
import { DeleteFileDialog, FileEditor } from "@/areas/code/editor";
import { FileTree } from "@/areas/code/file-tree";
import { contentsQuery, resolveQuery, treeCommitsQuery } from "@/areas/code/queries";
import {
  Breadcrumbs,
  CopyButton,
  LatestCommit,
  RefKind,
  RefPicker,
  joinPath,
  useSwitchRef,
} from "@/areas/code/refs";
import { Region } from "@/components/region";
import { Empty, EmptyDescription, EmptyHeader, EmptyTitle } from "@/components/ui/empty";
import { Skeleton } from "@/components/ui/skeleton";

// One file at a ref, with its editor and delete dialog. ?edit opens the editor.

export const Route = createFileRoute("/$owner/$repo/blob/$")({
  validateSearch: (search: Record<string, unknown>): { edit?: true } =>
    search.edit ? { edit: true } : {},
  loader: async ({ context, params }) => {
    const { owner, repo, _splat = "" } = params;
    const hit = await context.queryClient
      .ensureQueryData(resolveQuery({ owner, repo, splat: _splat }))
      .catch(() => undefined);
    if (hit?.type === "file") {
      void context.queryClient.prefetchQuery(
        contentsQuery({ owner, repo, ref: hit.ref, path: hit.path }),
      );
    }
  },
  component: BlobPage,
});

function BlobPage() {
  const { owner, repo, _splat = "" } = Route.useParams();
  return (
    <Region name="this file" fallback={<Skeleton className="h-64" />}>
      <BlobBody owner={owner} repo={repo} splat={_splat} />
    </Region>
  );
}

function BlobBody({ owner, repo, splat }: { owner: string; repo: string; splat: string }) {
  const { data: at } = useSuspenseQuery(resolveQuery({ owner, repo, splat }));
  const search = Route.useSearch();
  const navigate = Route.useNavigate();
  const [deleting, setDeleting] = useState(false);
  const switchRef = useSwitchRef("blob", { owner, repo }, at.path);
  const editing = Boolean(search.edit) && at.kind === "branch";
  if (at.type !== "file") {
    return (
      <Empty className="border py-12">
        <EmptyHeader>
          <EmptyTitle>
            {at.type === "dir" ? `${at.path || repo} is a directory` : `No file at ${at.path}`}
          </EmptyTitle>
          <EmptyDescription>
            <Link
              to="/$owner/$repo/tree/$"
              params={{ owner, repo, _splat: at.type === "dir" ? splat : at.ref }}
              className="text-primary hover:underline"
            >
              {at.type === "dir" ? "Browse it" : `Browse ${at.ref}`}
            </Link>
          </EmptyDescription>
        </EmptyHeader>
      </Empty>
    );
  }
  return (
    <div className="grid items-start gap-4 lg:grid-cols-[16rem_minmax(0,1fr)]">
      <aside className="bg-card hidden max-h-[75vh] overflow-y-auto rounded-sm border p-1 lg:block">
        <Region name="file tree" rows={8}>
          <FileTree owner={owner} repo={repo} ref={at.ref} sha={at.sha} path={at.path} />
        </Region>
      </aside>
      <div className="flex min-w-0 flex-col gap-3">
        <div className="flex flex-wrap items-center gap-2">
          <RefPicker owner={owner} repo={repo} current={at.ref} onPick={switchRef} />
          <RefKind kind={at.kind} />
          <Breadcrumbs owner={owner} repo={repo} ref={at.ref} path={at.path} />
          <CopyButton value={at.path} label="Copy path" />
        </div>
        <Region name="latest commit" fallback={<Skeleton className="h-9" />}>
          <FileCommit owner={owner} repo={repo} refName={at.ref} path={at.path} />
        </Region>
        <Region name="file contents" rows={12}>
          {editing ? (
            <Editing
              owner={owner}
              repo={repo}
              branch={at.ref}
              path={at.path}
              onClose={() => void navigate({ search: {}, replace: true })}
            />
          ) : (
            <>
              <BlobView
                owner={owner}
                repo={repo}
                refName={at.ref}
                path={at.path}
                editable={at.kind === "branch"}
                onEdit={() => void navigate({ search: { edit: true }, hash: "" })}
                onDelete={() => setDeleting(true)}
              />
              {at.kind === "branch" && (
                <DeleteDialog
                  owner={owner}
                  repo={repo}
                  branch={at.ref}
                  path={at.path}
                  open={deleting}
                  onOpenChange={setDeleting}
                />
              )}
            </>
          )}
        </Region>
      </div>
    </div>
  );
}

function useFile(owner: string, repo: string, ref: string, path: string) {
  const { data } = useSuspenseQuery(contentsQuery({ owner, repo, ref, path }));
  if (Array.isArray(data) || data.type !== "file") throw new Error(`${path} is not a file`);
  return data as Schemas["content-file"];
}

function Editing({
  owner,
  repo,
  branch,
  path,
  onClose,
}: {
  owner: string;
  repo: string;
  branch: string;
  path: string;
  onClose: () => void;
}) {
  const file = useFile(owner, repo, branch, path);
  const text = decodeText(file.content);
  if (text === null) throw new Error(`${path} is binary and cannot be edited here`);
  return (
    <FileEditor
      owner={owner}
      repo={repo}
      branch={branch}
      file={{ ...file, text }}
      onClose={onClose}
    />
  );
}

function DeleteDialog({
  owner,
  repo,
  branch,
  path,
  open,
  onOpenChange,
}: {
  owner: string;
  repo: string;
  branch: string;
  path: string;
  open: boolean;
  onOpenChange: (o: boolean) => void;
}) {
  const file = useFile(owner, repo, branch, path);
  return (
    <DeleteFileDialog
      owner={owner}
      repo={repo}
      branch={branch}
      file={file}
      open={open}
      onOpenChange={onOpenChange}
    />
  );
}

function FileCommit({
  owner,
  repo,
  refName,
  path,
}: {
  owner: string;
  repo: string;
  refName: string;
  path: string;
}) {
  const dir = path.includes("/") ? path.slice(0, path.lastIndexOf("/")) : "";
  const { data } = useSuspenseQuery(treeCommitsQuery({ owner, repo, ref: refName, path: dir }));
  const entry = data.entries.find((e) => e.path === path);
  if (!entry) return null;
  return (
    <div className="overflow-hidden rounded-sm border">
      <LatestCommit
        owner={owner}
        repo={repo}
        commit={entry.commit}
        historyTo={joinPath(refName, path)}
      />
    </div>
  );
}
