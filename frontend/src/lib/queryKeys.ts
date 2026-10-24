// TODO(ai-review): review for style and correctness
// Mirrored by `backend/src/preload.rs` for server-side preloads; `queryKeys.contract.json` pins both.
import type { AppId, ManifestRef } from "../api";

export const queryKeys = {
  manifestInfo: (appid: AppId, depotId: number, manifestId: string, branch: string) =>
    ["manifest-info", appid, depotId, manifestId, branch] as const,
  manifestFiles: (appid: AppId, depotId: number, manifestId: string, branch: string) =>
    ["manifest-files", appid, depotId, manifestId, branch] as const,
  manifestStatuses: (appid: AppId, refs: ManifestRef[]) =>
    ["manifest-statuses", appid, refs] as const,
};
