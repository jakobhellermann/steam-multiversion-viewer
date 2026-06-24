import type { GameInfo } from "../api";

/// "<game name>-<version>", falling back to the manifest id when no game
/// version was detected.
export function exportFolderName(
  appName: string | undefined,
  appid: number,
  gameInfo: GameInfo | undefined,
  manifestId: string,
): string {
  const name = appName?.trim() || `App ${appid}`;
  const version = gameInfo?.engine?.data.bundle_version?.trim();
  return pathSafe(`${name}-${version || manifestId}`);
}

/// Folds everything Windows or POSIX would choke on into `-`. Trailing dots and
/// spaces go too — Windows strips them silently, which would leave us with a
/// directory whose name differs from the one we reported.
function pathSafe(name: string): string {
  const cleaned = name
    // eslint-disable-next-line no-control-regex
    .replace(/[\x00-\x1f<>:"/\\|?*]+/g, "-")
    .replace(/ *- */g, "-")
    .replace(/-{2,}/g, "-")
    .replace(/^[-. ]+|[-. ]+$/g, "");
  return cleaned || "export";
}

/// Display-only join for the export path tooltip; the separator follows
/// whatever the configured root already uses.
export function joinExportPath(root: string, name: string): string {
  const separator = root.includes("\\") && !root.includes("/") ? "\\" : "/";
  return `${root.replace(/[/\\]+$/, "")}${separator}${name}`;
}
