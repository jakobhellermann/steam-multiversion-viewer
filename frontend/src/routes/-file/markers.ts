// TODO(ai-review): review for style and correctness
//
// Wire-format markers shared with the backend's `unity::markers`
// module. The backend emits each marker as a JSON string
//
//   __MARK__<type>␞<payload>
//
// inside the pretty-printed object dump. One regex on the post-shiki
// HTML matches every marker, then `renderMarker` dispatches on the
// type tag.
//
// Adding a new marker means three places: a type tag + format helper
// on the backend, an arm in the dump walker, and a matching case in
// `renderMarker` below.
//
// Currently shipped markers:
//
// - `pptr` — `__MARK__pptr␞<ref>␞<target>␞<type>␞<file>` (Unity PPtr).
// - `color` — `__MARK__color␞#rrggbbaa` (rgba color).

import type { FileLocator } from "./types";

const MARK_PREFIX = "__MARK__";
const MARK_SEP = "␞";
// `[^"]*` for the payload is enough because JSON strings can't contain
// raw `"` (it'd close the string) and `␞` is JSON-stringify-stable —
// shiki keeps the whole marker inside one string token.
const MARKER_RE = new RegExp(`"${MARK_PREFIX}([a-z]+)${MARK_SEP}([^"]*)"`, "g");

/// Compose a post-shiki HTML rewrite that swaps each marker for its
/// rendered HTML. Unknown types pass through untouched so a backend
/// that ships a marker before the frontend understands it doesn't
/// crash the page.
export function makePostProcess(locator: FileLocator): (html: string) => string {
  const renderPptr = makePptrRenderer(locator);
  return (html) =>
    html.replace(MARKER_RE, (whole, type, payload) => {
      switch (type) {
        case "pptr":
          return renderPptr(payload);
        case "color":
          return renderColor(payload);
        default:
          return whole;
      }
    });
}

function makePptrRenderer(locator: FileLocator) {
  const branchParam =
    locator.branch === "public" ? "" : `&branch=${encodeURIComponent(locator.branch)}`;
  const fileHref = (depotPath: string) =>
    `/apps/${locator.appid}/depots/${locator.depotId}/manifests/${locator.manifestId}/file?path=${encodeURIComponent(depotPath)}${branchParam}`;
  // Strip the `<Game>_Data/` data-dir prefix from file labels — the
  // user already knows which game/version they're in, so the extra
  // prefix only wastes horizontal space.
  const dataDirPrefix = (() => {
    const slash = locator.path.indexOf("/");
    return slash > 0 ? locator.path.slice(0, slash + 1) : "";
  })();
  const shortFileLabel = (depotPath: string) =>
    dataDirPrefix && depotPath.startsWith(dataDirPrefix)
      ? depotPath.slice(dataDirPrefix.length)
      : depotPath;
  return (payload: string): string => {
    const [ref = "", target = "", type = "", file = ""] = payload.split(MARK_SEP);
    // Fully-empty payload = null pptr that landed in a map-key
    // position. Render as plain `null` to avoid a stray "()".
    if (!ref && !target && !type && !file) {
      return '<span class="text-slate-500">null</span>';
    }
    // Label depends on locality: local refs show the target's name
    // (resolved by the backend), external ones show the depot path
    // of the file they live in — the latter is what's actually
    // identifying since cross-file targets often have no `m_Name`.
    const labelText = file ? shortFileLabel(file) : target;
    const label = escHTML(labelText) || '<span class="text-slate-500">null</span>';
    const ty = escHTML(type);
    // If we couldn't recover a real name (backend fell back to
    // `PathID=N`), keep that as a hint next to the type so the row
    // still tells you *which* `Shader` you're looking at.
    const pathHint =
      file && /^PathID=\d+$/.test(target)
        ? ` <span class="text-slate-500">${escHTML(target)}</span>`
        : "";
    const suffix = `${ty ? ` <span class="text-slate-500">(${ty})</span>` : ""}${pathHint}`;
    if (!ref) {
      return `<span class="text-slate-400">${label}</span>${suffix}`;
    }
    // Same shape for local + external — the click handler dispatches:
    // hash-only refs stay inside this file (no `href`), while external
    // refs carry a real `href` so middle-/ctrl-click still open in a
    // new tab and the click handler can route via tanstack-router.
    const linkAttrs = file
      ? `href="${escHTML(fileHref(file))}#${escHTML(ref)}" data-pptr-ref="${escHTML(ref)}"`
      : `data-pptr-ref="${escHTML(ref)}"`;
    return `<a ${linkAttrs} class="cursor-pointer text-sky-400 underline decoration-sky-700 hover:decoration-sky-400 hover:text-sky-200">${label}</a>${suffix}`;
  };
}

function renderColor(payload: string): string {
  // The backend constrains the payload to `#` + 8 lower-case hex
  // chars; bail out on anything else so a malformed marker can't
  // smuggle attribute-breaking characters into the inline style.
  if (!/^#[0-9a-f]{8}$/.test(payload)) {
    return `<span class="text-slate-400">${escHTML(payload)}</span>`;
  }
  return `<span class="inline-block h-[0.9em] w-[0.9em] mr-1 rounded-sm border border-slate-700/60" style="background-color: ${payload}; vertical-align: -0.08em"></span><span class="text-slate-300">${payload}</span>`;
}

export function escHTML(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}
