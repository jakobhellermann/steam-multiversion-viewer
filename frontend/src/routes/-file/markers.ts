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
// - `pptr` — `__MARK__pptr␞<ref>␞<target>␞<type>␞<file>␞<side>` (Unity PPtr).
//   `side` is `""` for non-diff dumps, `"base"`/`"target"` inside the
//   structured-diff content endpoint so the renderer can resolve the
//   pptr against the right manifest.
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
///
/// `isLocalRefInTree` decides whether a same-file pptr ref is a real
/// jump target (rendered as a link) or a dead one (rendered as plain
/// text). The backend filters most dead refs (self-transforms, etc)
/// but some — like Transform refs on components when the tree only
/// lists GameObjects — still come through with the marker shape.
///
/// `sideLocators` is the diff-mode hook: when set, pptrs carrying a
/// `side=base|target` tag in their payload are resolved against the
/// matching locator instead of the default one. Pass `undefined`
/// outside the structured-diff content pane.
export function makePostProcess(
  locator: FileLocator,
  isLocalRefInTree: (ref: string) => boolean,
  sideLocators?: { base: FileLocator; target: FileLocator },
): (html: string) => string {
  const defaultRenderer = makePptrRenderer(locator, isLocalRefInTree);
  const baseRenderer = sideLocators
    ? makePptrRenderer(sideLocators.base, isLocalRefInTree)
    : defaultRenderer;
  const targetRenderer = sideLocators
    ? makePptrRenderer(sideLocators.target, isLocalRefInTree)
    : defaultRenderer;
  return (html) =>
    html.replace(MARKER_RE, (whole, type, payload) => {
      switch (type) {
        case "pptr": {
          // The trailing `␞<side>` field (added for diff dumps) tells us
          // which renderer to pick. Missing or empty → caller didn't
          // tag a side and we use the default locator.
          const side = payload.split(MARK_SEP)[4] ?? "";
          if (side === "base") return baseRenderer(payload);
          if (side === "target") return targetRenderer(payload);
          return defaultRenderer(payload);
        }
        case "color":
          return renderColor(payload);
        default:
          return whole;
      }
    });
}

function makePptrRenderer(locator: FileLocator, isLocalRefInTree: (ref: string) => boolean) {
  const branchParam =
    locator.branch === "public" ? "" : `&branch=${encodeURIComponent(locator.branch)}`;
  const fileHref = (depotPath: string) =>
    `/apps/${locator.appid}/depots/${locator.depotId}/manifests/${locator.manifestId}/file?path=${encodeURIComponent(depotPath)}${branchParam}`;
  // Only show the file's basename in the link label — the full depot
  // path (the `<Game>_Data/StreamingAssets/aa/StandaloneWindows64/…`
  // mouthful for addressables bundles) belongs in the `href` hover,
  // not on screen for every pptr in the dump.
  const shortFileLabel = (depotPath: string) => {
    const slash = depotPath.lastIndexOf("/");
    return slash >= 0 ? depotPath.slice(slash + 1) : depotPath;
  };
  return (payload: string): string => {
    const [ref = "", target = "", type = "", file = ""] = payload.split(MARK_SEP);
    // Fully-empty payload = null pptr that landed in a map-key
    // position. Render as plain `null` to avoid a stray "()".
    if (!ref && !target && !type && !file) {
      return '<span class="text-slate-500">null</span>';
    }
    // Label shape:
    //  - local refs: just the target name (the row in the tree they
    //    point at carries everything else).
    //  - external refs: `<file>: <name>` when the backend resolved a
    //    real `m_Name`; `<file> #<pathid>` otherwise. Either way the
    //    file is what makes the reference unambiguous.
    const ty = escHTML(type);
    const pathIdHint = (() => {
      const m = /^obj:(\d+)$/.exec(ref);
      return m ? ` <span class="text-slate-500">#${m[1]}</span>` : "";
    })();
    const hasResolvedName = target !== "" && !/^PathID=\d+$/.test(target);
    let label: string;
    let pathHint = "";
    if (file) {
      const shortFile = escHTML(shortFileLabel(file));
      if (hasResolvedName) {
        label = `${shortFile}<span class="text-slate-500">:</span> ${escHTML(target)}`;
      } else {
        label = shortFile;
        pathHint = pathIdHint;
      }
    } else {
      label = escHTML(target) || '<span class="text-slate-500">null</span>';
    }
    const suffix = `${ty ? ` <span class="text-slate-500">(${ty})</span>` : ""}${pathHint}`;
    if (!ref) {
      return `<span class="text-slate-400">${label}</span>${suffix}`;
    }
    // Same-file refs to objects the tree doesn't list (e.g. Transform
    // components when the tree only carries GameObjects) have no jump
    // target — render plain so the user doesn't chase a dead click.
    if (!file && !isLocalRefInTree(ref)) {
      return `<span class="text-slate-400">${label}</span>${suffix}`;
    }
    // Both local and external refs get a real `href` so browsers show
    // the target on hover and middle-/ctrl-click opens a new tab. The
    // click handler tells them apart via `data-pptr-file`: same-file
    // refs go through the in-page hash logic, external refs through
    // tanstack-router.
    const linkAttrs = file
      ? `href="${escHTML(fileHref(file))}#${escHTML(ref)}" data-pptr-ref="${escHTML(ref)}" data-pptr-file="1"`
      : `href="#${escHTML(ref)}" data-pptr-ref="${escHTML(ref)}"`;
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
