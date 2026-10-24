// TODO(ai-review): review for style and correctness
import { queryOptions, useQuery, type UseQueryResult } from "@tanstack/react-query";

import { fetchLibrary, type OwnedGame } from "./api";
import { queryKeys } from "./queryKeys";

/// Preloaded owned games can be a last-known copy; once older than the backend's HTTP max-age
/// they are refetched in the background.
export const libraryQuery = queryOptions({
  queryKey: queryKeys.library(),
  queryFn: fetchLibrary,
  staleTime: 5 * 60 * 1000,
});

export function useLibrary(): UseQueryResult<OwnedGame[]> {
  return useQuery({ ...libraryQuery, refetchOnMount: true });
}
