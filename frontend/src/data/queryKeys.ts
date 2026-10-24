// TODO(ai-review): review for style and correctness
// Mirrored by `backend/src/preload.rs` for server-side preloads; `queryKeys.contract.json` pins both.
import type { AppId, ManifestRef } from "./api";

export const queryKeys = {
  library: () => ["library"] as const,
  auth: () => ["auth"] as const,
  mountStatus: () => ["mount-status"] as const,
  exportStatus: () => ["export-status"] as const,
  config: () => ["config"] as const,
  app: (appid: AppId) => ["app", appid] as const,
  extraManifests: (appid: AppId) => ["extra-manifests", appid] as const,
  manifestInfo: (appid: AppId, depotId: number, manifestId: string, branch: string) =>
    ["manifest-info", appid, depotId, manifestId, branch] as const,
  manifestFiles: (appid: AppId, depotId: number, manifestId: string, branch: string) =>
    ["manifest-files", appid, depotId, manifestId, branch] as const,
  gameInfo: (appid: AppId, depotId: number, manifestId: string) =>
    ["game-info", appid, depotId, manifestId] as const,
  manifestStatuses: (appid: AppId, refs: ManifestRef[]) =>
    ["manifest-statuses", appid, refs] as const,
};
