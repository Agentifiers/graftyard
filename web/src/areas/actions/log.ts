import { useEffect, useReducer, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { ansiToLines, type CodeBlockToken } from "@/components/reui/code-block/code-block";
import type { Schemas } from "@/api/client";
import { jobLogQuery, jobQuery, jobsQuery, type SavedLine } from "./queries";
import { relayUrl, type RelayLine, type RelayMessage } from "./ui-api";

// A job's log as the viewer renders it: per step, its lines, with `##[group]` folds and
// `##[error]`-style commands parsed out. Fed either by the LogRelay or by the saved blob.

export type LogLine = {
  // 1-based within its step, as line links number them.
  n: number;
  at: string;
  text: string;
  tokens: CodeBlockToken[];
  kind: "plain" | "command" | "error" | "warning" | "notice";
  // The group this line opens or sits in (index into the step's groups).
  group?: number;
  opensGroup?: boolean;
};

export type StepLog = { number: number; lines: LogLine[]; groups: string[] };

const COMMAND = /^##\[(group|endgroup|error|warning|notice|command)\](.*)$/;

// Pure: raw lines of one step to rendered lines.
export function parseStep(raw: { at: string; text: string }[]): Omit<StepLog, "number"> {
  const lines: LogLine[] = [];
  const groups: string[] = [];
  let open: number | undefined;
  for (const { at, text } of raw) {
    const m = COMMAND.exec(text);
    const tag = m?.[1];
    if (tag === "endgroup") {
      open = undefined;
      continue;
    }
    const body = m ? m[2] : text.startsWith("[command]") ? text.slice(9) : text;
    const parsed = ansiToLines(body)[0];
    const line: LogLine = {
      n: lines.length + 1,
      at,
      text: parsed?.text ?? body,
      tokens: parsed?.tokens ?? [],
      kind:
        tag === "error" || tag === "warning" || tag === "notice"
          ? tag
          : tag === "command" || text.startsWith("[command]")
            ? "command"
            : "plain",
    };
    if (tag === "group") {
      groups.push(line.text);
      open = groups.length - 1;
      line.group = open;
      line.opensGroup = true;
    } else if (open !== undefined) line.group = open;
    lines.push(line);
  }
  return { lines, groups };
}

// Saved blob lines carry no step number; the job's steps' times assign them.
export function splitByStep(
  lines: SavedLine[],
  steps: NonNullable<Schemas["job"]["steps"]>,
): RelayLine[] {
  const ranked = steps.filter((s) => s.started_at);
  return lines.map((l, seq) => {
    let step = ranked[0]?.number ?? 1;
    for (const s of ranked) if (s.started_at && l.at >= s.started_at) step = s.number;
    return { seq, step, at: l.at, text: l.text };
  });
}

export type Source = "connecting" | "live" | "saved";

type State = { source: Source; lines: RelayLine[]; fallback: boolean; error?: string };
type Action =
  | { type: "live"; tail: RelayLine[] }
  | { type: "line"; line: RelayLine }
  | { type: "saved"; lines: RelayLine[]; fallback: boolean }
  | { type: "error"; message: string };

function reduce(s: State, a: Action): State {
  switch (a.type) {
    case "live":
      return { source: "live", lines: mergeLines(s.lines, a.tail), fallback: false };
    case "line":
      return { ...s, lines: mergeLines(s.lines, [a.line]) };
    case "saved":
      return { source: "saved", lines: a.lines, fallback: a.fallback };
    // A failed read keeps whatever is already shown; it only matters when nothing is.
    case "error":
      return { ...s, error: a.message };
  }
}

// Lines are keyed by seq, so a tail that overlaps what is already shown adds nothing twice.
function mergeLines(have: RelayLine[], more: RelayLine[]) {
  if (more.length === 0) return have;
  const next = have.slice();
  for (const l of more) next[l.seq] = l;
  return next.filter(Boolean);
}

type Ref = { owner: string; repo: string; runId: number; jobId: number };

// Joins the job's relay while it runs. When the relay has no runner stream, closes, or
// errors, the hook reads the saved blob instead, and `fallback` says the live view was
// expected but unavailable. A completed job reads the blob directly.
export function useJobLog(ref: Ref, job: Schemas["job"]) {
  const queryClient = useQueryClient();
  const running = job.status === "in_progress";
  const [state, dispatch] = useReducer(reduce, {
    source: "connecting",
    lines: [],
    fallback: false,
  });

  const [attempt, setAttempt] = useState(0);

  useEffect(() => {
    let closed = false;
    const readSaved = async (fallback: boolean) => {
      const options = jobLogQuery(ref);
      await queryClient.invalidateQueries({ queryKey: options.queryKey, exact: true });
      const saved = await queryClient.fetchQuery(options).catch((e: unknown) => {
        if (!closed)
          dispatch({ type: "error", message: e instanceof Error ? e.message : String(e) });
        return null;
      });
      const fresh = await queryClient.fetchQuery(jobQuery(ref)).catch(() => job);
      if (!closed && saved)
        dispatch({ type: "saved", lines: splitByStep(saved, fresh.steps ?? []), fallback });
    };
    if (!running) {
      void readSaved(false);
      return () => void (closed = true);
    }
    const socket = new WebSocket(relayUrl(ref.jobId));
    let ended = false;
    let poll: ReturnType<typeof setInterval> | undefined;
    // Without the relay, a job that is still running keeps writing its blob; reread it.
    const fallBack = () => {
      if (ended) return;
      ended = true;
      void readSaved(true);
      poll = setInterval(() => void readSaved(true), 3000);
    };
    socket.onmessage = (e) => {
      const m = JSON.parse(String(e.data)) as RelayMessage;
      if (m.type === "hello") dispatch({ type: "live", tail: m.tail });
      else if (m.type === "line") dispatch({ type: "line", line: m.line });
      else if (m.type === "step")
        void queryClient.invalidateQueries({ queryKey: jobQuery(ref).queryKey });
      else if (m.type === "end") {
        ended = true;
        void queryClient.invalidateQueries({ queryKey: jobQuery(ref).queryKey });
        void queryClient.invalidateQueries({ queryKey: jobsQuery(ref).queryKey });
        void readSaved(false);
      } else if (m.type === "no-stream") fallBack();
    };
    socket.onerror = fallBack;
    socket.onclose = () => {
      if (!closed) fallBack();
    };
    return () => {
      closed = true;
      clearInterval(poll);
      socket.close();
    };
    // Reconnect only when the job starts or stops running, not on every poll.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [ref.jobId, running, job.run_attempt, attempt]);

  return { ...state, retry: () => setAttempt((n) => n + 1) };
}

export function byStep(lines: RelayLine[]): Map<number, StepLog> {
  const raw = new Map<number, RelayLine[]>();
  for (const l of lines) raw.set(l.step, [...(raw.get(l.step) ?? []), l]);
  return new Map([...raw].map(([number, ls]) => [number, { number, ...parseStep(ls) }]));
}
