// TODO(ai-review): review for style and correctness
import { expect, test } from "vitest";
import contract from "./queryKeys.contract.json";
import { queryKeys } from "./queryKeys";

test("query keys match the backend preload contract", () => {
  const [appid, depotId, manifestId, branch] = [1030300, 1030303, "5954103139200615141", "beta"];
  expect({
    library: queryKeys.library(),
    auth: queryKeys.auth(),
    mountStatus: queryKeys.mountStatus(),
    exportStatus: queryKeys.exportStatus(),
    config: queryKeys.config(),
    app: queryKeys.app(appid),
    extraManifests: queryKeys.extraManifests(appid),
    manifestInfo: queryKeys.manifestInfo(appid, depotId, manifestId, branch),
    manifestFiles: queryKeys.manifestFiles(appid, depotId, manifestId, branch),
    gameInfo: queryKeys.gameInfo(appid, depotId, manifestId),
    manifestStatuses: queryKeys.manifestStatuses(appid, [
      { depot_id: depotId, manifest_id: manifestId, branch },
    ]),
  }).toEqual(contract);
});
