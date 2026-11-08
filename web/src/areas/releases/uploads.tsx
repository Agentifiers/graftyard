import { useCallback, useRef, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { HugeiconsIcon } from "@hugeicons/react";
import { Cancel01Icon, CloudUploadIcon, Refresh01Icon } from "@hugeicons/core-free-icons";
import { toast } from "sonner";
import { useFileUpload, type FileWithPreview } from "@/hooks/use-file-upload";
import { FileType, getFileType } from "@/components/reui/file-type";
import { Button } from "@/components/ui/button";
import { Progress } from "@/components/ui/progress";
import { cn } from "cn";
import { errorText, fetchWithProgress, send, uploads, WriteError } from "./api";
import { formatSize } from "./parts";
import { releasesKey } from "./queries";

type Repo = { owner: string; repo: string };

// One file on its way to a release: waiting for the release to exist, sending, or failed.
// A finished upload leaves this list; the asset then shows in the release's asset list.
export type Upload =
  | { id: string; file: File; phase: "queued" }
  | { id: string; file: File; phase: "sending"; progress: number }
  | { id: string; file: File; phase: "failed"; error: string };

export function useAssetUploads(repo: Repo) {
  const queryClient = useQueryClient();
  const [items, setItems] = useState<Upload[]>([]);
  const patch = (id: string, next: Upload) =>
    setItems((list) => list.map((u) => (u.id === id ? next : u)));

  // Sends one file and reports whether it landed. Never retried: the user retries a row.
  const send1 = useCallback(
    async (releaseId: number, u: Upload) => {
      patch(u.id, { id: u.id, file: u.file, phase: "sending", progress: 0 });
      try {
        await send(
          uploads.POST("/repos/{owner}/{repo}/releases/{release_id}/assets", {
            params: { path: { ...repo, release_id: releaseId }, query: { name: u.file.name } },
            body: u.file,
            bodySerializer: (b) => b,
            headers: { "Content-Type": u.file.type || "application/octet-stream" },
            fetch: fetchWithProgress((progress) =>
              patch(u.id, { id: u.id, file: u.file, phase: "sending", progress }),
            ),
          }),
        );
        setItems((list) => list.filter((x) => x.id !== u.id));
        void queryClient.invalidateQueries({ queryKey: releasesKey(repo) });
        return true;
      } catch (e) {
        const error =
          e instanceof WriteError && e.fields.name
            ? `An asset named ${u.file.name} ${e.fields.name}.`
            : errorText(e);
        patch(u.id, { id: u.id, file: u.file, phase: "failed", error });
        toast.error(`${u.file.name} not uploaded`, { description: error });
        return false;
      }
    },
    [repo.owner, repo.repo, queryClient],
  );

  const add = (files: FileWithPreview[]) => {
    const added: Upload[] = files.flatMap((f) =>
      f.file instanceof File ? [{ id: f.id, file: f.file, phase: "queued" as const }] : [],
    );
    setItems((list) => [...list, ...added]);
    return added;
  };

  // Uploads every file still waiting or failed, one at a time, so progress reads in order.
  const sendAll = async (releaseId: number, list: Upload[] = items) => {
    let ok = true;
    for (const u of list.filter((x) => x.phase !== "sending")) {
      ok = (await send1(releaseId, u)) && ok;
    }
    return ok;
  };

  return {
    items,
    add,
    send: send1,
    sendAll,
    remove: (id: string) => setItems((list) => list.filter((u) => u.id !== id)),
    busy: items.some((u) => u.phase === "sending"),
  };
}

// The drop zone and the rows of files in flight. With `releaseId`, a dropped file uploads
// at once; without it (a release not created yet), files wait for the form's save.
export function AssetDropzone({
  uploads: up,
  releaseId,
}: {
  uploads: ReturnType<typeof useAssetUploads>;
  releaseId: number | undefined;
}) {
  const releaseRef = useRef(releaseId);
  releaseRef.current = releaseId;
  const [state, actions] = useFileUpload({
    multiple: true,
    maxSize: 100 * 1024 * 1024,
    onFilesAdded: (files) => {
      const added = up.add(files);
      const id = releaseRef.current;
      if (id !== undefined) void up.sendAll(id, added);
      // The hook keeps its own list for duplicate detection; this list is the source of truth.
      setTimeout(() => actions.clearFiles(), 0);
    },
  });

  return (
    <div className="flex flex-col gap-2">
      <div
        role="button"
        tabIndex={0}
        data-testid="dropzone"
        data-dragging={state.isDragging || undefined}
        onDragEnter={actions.handleDragEnter}
        onDragLeave={actions.handleDragLeave}
        onDragOver={actions.handleDragOver}
        onDrop={actions.handleDrop}
        onClick={actions.openFileDialog}
        onKeyDown={(e) => {
          if (e.key === "Enter" || e.key === " ") actions.openFileDialog();
        }}
        className={cn(
          "text-muted-foreground hover:bg-muted/40 flex cursor-pointer flex-col items-center gap-1 rounded-sm border border-dashed px-4 py-6 text-center text-sm",
          state.isDragging && "border-primary bg-primary/5 text-foreground",
        )}
      >
        <HugeiconsIcon icon={CloudUploadIcon} strokeWidth={2} className="size-6" />
        <span>
          Attach binaries by dropping them here or{" "}
          <span className="text-primary font-medium">selecting them</span>.
        </span>
        <span className="text-xs">
          {releaseId === undefined
            ? "Files upload when you save the release."
            : "Files upload as soon as you drop them."}
        </span>
        <input {...actions.getInputProps()} className="sr-only" aria-label="Attach binaries" />
      </div>
      {state.errors.length > 0 && (
        <p className="text-destructive text-xs">{state.errors.join(" ")}</p>
      )}
      {up.items.length > 0 && (
        <ul className="divide-y border text-sm" data-testid="uploads">
          {up.items.map((u) => (
            <li
              key={u.id}
              className="flex flex-col gap-1 px-3 py-1.5"
              data-upload={u.file.name}
              data-phase={u.phase}
            >
              <div className="flex items-center gap-2">
                <FileType type={getFileType(u.file.name)} size="mini" />
                <span className="min-w-0 flex-1 truncate font-medium">{u.file.name}</span>
                <span className="text-muted-foreground text-xs tabular-nums">
                  {formatSize(u.file.size)}
                </span>
                {u.phase === "sending" && (
                  <span className="text-muted-foreground w-24 text-right text-xs tabular-nums">
                    {u.progress < 1 ? `${Math.round(u.progress * 100)}%` : "Processing…"}
                  </span>
                )}
                {u.phase === "queued" && (
                  <span className="text-muted-foreground w-24 text-right text-xs">Waiting</span>
                )}
                {u.phase === "failed" && releaseId !== undefined && (
                  <Button size="xs" variant="outline" onClick={() => void up.send(releaseId, u)}>
                    <HugeiconsIcon icon={Refresh01Icon} strokeWidth={2} data-icon="inline-start" />
                    Retry
                  </Button>
                )}
                {u.phase !== "sending" && (
                  <Button
                    size="icon-xs"
                    variant="ghost"
                    aria-label={`Remove ${u.file.name}`}
                    onClick={() => up.remove(u.id)}
                  >
                    <HugeiconsIcon icon={Cancel01Icon} strokeWidth={2} />
                  </Button>
                )}
              </div>
              {u.phase === "sending" && (
                <Progress
                  value={Math.round(u.progress * 100)}
                  aria-label={`${u.file.name} upload`}
                />
              )}
              {u.phase === "failed" && (
                <p className="text-destructive text-xs" role="alert">
                  {u.error}
                </p>
              )}
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
