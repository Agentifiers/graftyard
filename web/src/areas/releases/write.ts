import { useMutation, useQueryClient, type QueryKey } from "@tanstack/react-query";
import { toast } from "sonner";
import { errorText } from "./api";

// A write that shows at once. `apply` edits the cached read named by `key` before the
// request leaves; a failure puts the snapshot back and toasts. Afterwards every read under
// `invalidate` refetches. Mutations never retry (src/api/query-client.ts).
export function useOptimisticWrite<TVars, TData, TResult>({
  key,
  apply,
  write,
  invalidate,
  failure,
  onSuccess,
}: {
  key: QueryKey;
  apply: (data: TData, vars: TVars) => TData;
  write: (vars: TVars) => Promise<TResult>;
  invalidate: QueryKey[];
  failure: string;
  onSuccess?: (result: TResult, vars: TVars) => void;
}) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: write,
    onMutate: async (vars: TVars) => {
      await queryClient.cancelQueries({ queryKey: key, exact: true });
      const before = queryClient.getQueryData<TData>(key);
      if (before !== undefined) queryClient.setQueryData<TData>(key, apply(before, vars));
      return { before };
    },
    onError: (e, _vars, ctx) => {
      if (ctx?.before !== undefined) queryClient.setQueryData(key, ctx.before);
      toast.error(failure, { description: errorText(e) });
    },
    onSuccess,
    onSettled: () =>
      Promise.all(invalidate.map((queryKey) => queryClient.invalidateQueries({ queryKey }))),
  });
}
