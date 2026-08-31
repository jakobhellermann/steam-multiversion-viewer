// TODO(ai-review): review for style and correctness

export type ParsedExtra = {
  manifest_id: string;
  branch: string;
  /// App ID extracted from the line, if the format provides one
  /// (depotdownloader / steam-console). Lets the modal warn when the
  /// pasted line references a different app than the user is on.
  app_id: number | null;
  /// Depot ID extracted from the line, same caveat as `app_id`.
  depot_id: number | null;
};

const BRANCH_TOKEN_RE = /[A-Za-z0-9_.-]+/g;

// What we accept after the manifest_id on a line as a branch label. Steam
// allows arbitrary branch names, so we don't try to whitelist — just
// reject pure-number noise (e.g. trailing column counts) and very short
// tokens that are almost certainly stray punctuation.
function looksLikeBranchToken(v: string): boolean {
  if (/^\d+$/.test(v)) return false;
  if (v.length < 2) return false;
  return true;
}

/// Manifest IDs are 17–20 digit base-10 numbers (truncated u64), depot/app
/// IDs sit in u32 range so we use a different length window to avoid
/// confusing them with manifest IDs.
const MANIFEST_ID_RE = /\b\d{15,20}\b/;
const SMALL_ID_RE = /\b\d{1,10}\b/g;

/// Pull manifest entries out of a textarea paste. Supports several
/// formats commonly used to reference Steam manifests:
///
///   * SteamDB manifest-history row:  `<date> <relative> <manifest_id> [branch]`
///   * depotdownloader CLI:           `-app N -depot N -manifest N [-beta BRANCH]`
///   * Steam console "download_depot": `download_depot APP DEPOT MANIFEST`
///   * Bare manifest ID, one per line.
///
/// Branches default to "public" when not present in the input.
/// SteamDB only serves the full manifest history to logged-in accounts;
/// anonymous visitors get the most recent ~10 rows and a "Please sign in
/// via Steam to view more entries" gate. When that gate text rides along
/// in the paste the user almost certainly copied a truncated history, so
/// we surface a warning rather than silently importing a partial list.
export function steamDbSignInGated(text: string): boolean {
  return /sign in via steam to view more/i.test(text);
}

export function parseSteamDbPaste(text: string): ParsedExtra[] {
  const out: ParsedExtra[] = [];
  const seen = new Set<string>();
  for (const rawLine of text.split(/\r?\n/)) {
    const entry = parseLine(rawLine);
    if (!entry) continue;
    // SteamDB marker for manifests not attached to any branch; there is
    // no branch to mint a request code for, fetching always fails.
    if (entry.branch === "_steamdb_external_") continue;
    if (seen.has(entry.manifest_id)) continue;
    seen.add(entry.manifest_id);
    out.push(entry);
  }
  return out;
}

function parseLine(line: string): ParsedExtra | null {
  // depotdownloader style: -app N -depot N -manifest N [-beta BRANCH].
  // The flags can appear in any order and we accept extra noise.
  const dd = parseDepotDownloaderFlags(line);
  if (dd) return dd;

  // Steam console: `download_depot <app> <depot> <manifest> [<branch>]`.
  const dc = parseDownloadDepotCommand(line);
  if (dc) return dc;

  // SteamDB row (or bare manifest_id line): first id-shaped number wins;
  // following tokens on the same line are scanned for a branch label.
  const idMatch = line.match(MANIFEST_ID_RE);
  if (!idMatch) return null;
  const id = idMatch[0];
  const after = line.slice((idMatch.index ?? 0) + id.length);
  let branch = "public";
  for (const tok of after.matchAll(BRANCH_TOKEN_RE)) {
    if (looksLikeBranchToken(tok[0])) {
      branch = tok[0];
      break;
    }
  }
  return { manifest_id: id, branch, app_id: null, depot_id: null };
}

function parseDepotDownloaderFlags(line: string): ParsedExtra | null {
  if (!/-manifest\s+\d+/.test(line)) return null;
  const manifestM = line.match(/-manifest\s+(\d+)/);
  if (!manifestM) return null;
  const manifest_id = manifestM[1];
  if (manifest_id.length < 15) return null;
  const appM = line.match(/-app\s+(\d+)/);
  const depotM = line.match(/-depot\s+(\d+)/);
  // `-beta` is depotdownloader's branch flag; the value may quote-wrap
  // but bare token is the common case.
  const betaM = line.match(/-beta\s+([A-Za-z0-9_.-]+)/);
  return {
    manifest_id,
    branch: betaM ? betaM[1] : "public",
    app_id: appM ? Number(appM[1]) : null,
    depot_id: depotM ? Number(depotM[1]) : null,
  };
}

function parseDownloadDepotCommand(line: string): ParsedExtra | null {
  const m = line.match(/\bdownload_depot\b\s+(.*)$/i);
  if (!m) return null;
  // Pull bare positional numbers from the args. First three are
  // app/depot/manifest. A fourth quoted/bare token is the branch.
  const args = m[1];
  const small = [...args.matchAll(SMALL_ID_RE)].map((x) => x[0]);
  const big = args.match(MANIFEST_ID_RE);
  if (!big) return null;
  const manifest_id = big[0];
  // First two small numbers (before the manifest_id) are app/depot.
  const beforeManifest = args.slice(0, big.index ?? 0);
  const beforeSmall = [...beforeManifest.matchAll(SMALL_ID_RE)].map((x) => x[0]);
  const app_id = beforeSmall[0] ? Number(beforeSmall[0]) : small[0] ? Number(small[0]) : null;
  const depot_id = beforeSmall[1] ? Number(beforeSmall[1]) : small[1] ? Number(small[1]) : null;
  // Branch: any non-number token after the manifest_id.
  const after = args.slice((big.index ?? 0) + manifest_id.length);
  let branch = "public";
  for (const tok of after.matchAll(BRANCH_TOKEN_RE)) {
    if (looksLikeBranchToken(tok[0])) {
      branch = tok[0];
      break;
    }
  }
  return { manifest_id, branch, app_id, depot_id };
}
