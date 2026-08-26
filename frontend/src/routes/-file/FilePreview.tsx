// TODO(ai-review): review for style and correctness
import { useQuery } from "@tanstack/react-query";
import { memo, useMemo } from "react";

import { fetchTransformedFile, type FileView } from "../../api";
import { formatBytes } from "../../lib/format";
import { mediaKindForPath } from "../../lib/mediaKind";
import { highlight, langForMime, langForPath } from "../../lib/syntax";
import { escHTML } from "./markers";
import { MediaPlayer } from "./MediaPlayer";
import { StructuredView } from "./StructuredView";
import type { FileLocator } from "./types";

/// Render `src` as the native media element for `kind`.
export function MediaView({
  kind,
  src,
  alt,
  imgClassName,
}: {
  kind: "image" | "audio" | "video";
  src: string;
  alt?: string;
  /// Extra `<img>` classes; ignored for audio/video.
  imgClassName?: string;
}) {
  if (kind === "image") {
    // Functional layout only; the caller styles framing/background.
    return <img src={src} alt={alt} className={`max-w-full ${imgClassName ?? ""}`} />;
  }
  return <MediaPlayer kind={kind} src={src} />;
}

/// Render `view` as the user expects: image/audio/video by extension,
/// transformer output for known binaries (.dll, .so), syntax-highlighted
/// text otherwise. `rawSrc` is the URL to the raw bytes (used for media
/// elements); `locator` is needed for the transformer query.
export function FilePreview({
  view,
  rawSrc,
  locator,
  showHeader = true,
}: {
  view: FileView;
  /// URL to the raw bytes — needed for `<img>` / `<audio>` / `<video>`
  /// rendering of binary media files.
  rawSrc: string;
  /// Identity of the file inside its manifest. Used as the transformer
  /// query key so a .dll's decompile result can be cached.
  locator: FileLocator;
  showHeader?: boolean;
}) {
  if (view.kind !== "file") {
    return null;
  }
  const header = showHeader ? (
    <h2 className="mb-2 text-sm font-semibold text-slate-400">Preview</h2>
  ) : null;

  const sectionClass = showHeader ? "mt-6" : "";

  // Structured view wins over everything else — files that have one
  // (unity scenes, eventually bundles) are useless as raw text. It
  // renders its own "Preview" header alongside the match counter so
  // both pieces of info live in one row.
  if (view.structured) {
    // Take the remaining vertical space inside the file route so
    // StructuredView can fill it. Without `min-h-0` the flex item
    // would refuse to shrink below its intrinsic content height.
    return (
      <section className={`${sectionClass} flex min-h-0 flex-1 flex-col`}>
        <StructuredView locator={locator} showHeader={showHeader} />
      </section>
    );
  }

  const media = mediaKindForPath(view.path);
  if (media) {
    return (
      <section className={sectionClass}>
        {header}
        <MediaView
          kind={media}
          src={rawSrc}
          alt={view.path}
          imgClassName="rounded border border-slate-800 bg-slate-950"
        />
      </section>
    );
  }

  if (view.content_kind === "binary") {
    if (view.transformer != null) {
      return (
        <section className={sectionClass}>
          {header}
          <TransformedPreview locator={locator} />
        </section>
      );
    }
    return (
      <section className={sectionClass}>
        {header}
        <p className="text-sm text-slate-500">
          Binary file — {formatBytes(view.size)}. No inline preview.
        </p>
      </section>
    );
  } else if (view.content_kind === "too_large") {
    return (
      <section className={sectionClass}>
        {header}
        <p className="text-sm text-slate-500">
          File is {formatBytes(view.size)}; preview cap is {formatBytes(view.preview_cap_bytes)}.
        </p>
      </section>
    );
  } else if (view.content_kind === "text" && view.content != null) {
    return (
      <section className={sectionClass}>
        {header}
        <HighlightedPre code={view.content} lang={langForPath(view.path)} />
      </section>
    );
  }

  return null;
}

/// Wraps the `/api/.../file/transformed` endpoint
/// — runs transformer on first access (can take multiple seconds), then
/// reuses the cached result for instant repeats.
function TransformedPreview({ locator }: { locator: FileLocator }) {
  const query = useQuery({
    queryKey: [
      "file-transformed",
      locator.appid,
      locator.depotId,
      locator.manifestId,
      locator.branch,
      locator.path,
    ],
    queryFn: () =>
      fetchTransformedFile(
        locator.appid,
        locator.depotId,
        locator.manifestId,
        locator.branch,
        locator.path,
      ),
  });
  if (query.isPending) {
    return <p className="text-sm text-slate-500">Decompiling… (first run can take a while)</p>;
  }
  if (query.error) {
    return (
      <p className="text-sm text-red-300">
        Decompile failed: {query.error instanceof Error ? query.error.message : String(query.error)}
      </p>
    );
  }
  if (query.data == null) {
    return <p className="text-sm text-slate-500">Not transformable.</p>;
  }
  return <HighlightedPre code={query.data.text} lang={langForMime(query.data.mime)} />;
}

/// Shiki-rendered code block. Memoised so a rerender of the parent
/// route (e.g. when the URL `compare_to` set changes) doesn't force
/// React to diff a multi-megabyte `dangerouslySetInnerHTML` blob.
export const HighlightedPre = memo(function HighlightedPre({
  code,
  lang,
  bare = false,
  postProcess,
}: {
  code: string;
  lang: ReturnType<typeof langForPath>;
  /// When true, skip the rounded border + padding chrome — useful when
  /// the caller already wraps the content in a styled card so we don't
  /// nest borders.
  bare?: boolean;
  /// Last-mile transform on the rendered HTML string — used to inject
  /// link markup (PPtr refs, jump targets, etc) without re-running
  /// the (expensive) shiki tokenisation.
  postProcess?: (html: string) => string;
}) {
  // Shiki's `codeToHtml` is synchronous and tokenises the whole input
  // on the main thread — a 1 MB file freezes the tab for seconds.
  // Skip highlighting entirely for big inputs; the browser renders
  // plain `<pre>` of that size without trouble. Threshold picked from
  // a quick eyeball: ~100k chars highlights in well under 100ms here.
  const HIGHLIGHT_MAX_CHARS = 100_000;
  const tooBigForHighlight = code.length > HIGHLIGHT_MAX_CHARS;
  const html = useQuery({
    // Cache by the actual code text — a length + 64-char prefix used
    // to collide for inputs that differed only further in (e.g. raw
    // vs PPtr-collapsed JSON), making us serve a stale render.
    queryKey: ["syntax-highlight", lang, code],
    queryFn: () => (lang ? highlight(code, lang) : Promise.resolve(null)),
    enabled: lang != null && !tooBigForHighlight,
    staleTime: Infinity,
    gcTime: Infinity,
  });
  // `content-visibility: auto` lets the browser skip layout + paint
  // for off-screen chunks. Pair with `contain-intrinsic-size` so the
  // scrollbar thumb is stable when chunks haven't been measured yet —
  // 16px/line is a fine guess for the text-xs font.
  const lineCount = useMemo(() => code.split("\n").length, [code]);
  const cvStyle = {
    contentVisibility: "auto",
    containIntrinsicSize: `1px ${lineCount * 16}px`,
  } as React.CSSProperties;
  // Shiki emits its own <pre> with the theme background; wrap so our
  // own padding/border/scroll behavior stays consistent.
  if (html.data) {
    const chrome = bare
      ? "text-xs [&_pre]:m-0! [&_pre]:bg-transparent! [&_pre]:p-0! [&_pre]:min-w-max"
      : "overflow-x-auto rounded border border-slate-800 text-xs [&_pre]:m-0! [&_pre]:bg-slate-950! [&_pre]:p-3!";
    const rendered = postProcess ? postProcess(html.data) : html.data;
    return (
      <div className={chrome} style={cvStyle} dangerouslySetInnerHTML={{ __html: rendered }} />
    );
  }
  const chrome = bare
    ? "font-mono text-xs whitespace-pre"
    : "overflow-x-auto rounded border border-slate-800 bg-slate-950 p-3 font-mono text-xs whitespace-pre";
  // Pre-shiki / too-big-for-shiki fallback. Escape first, then run
  // `postProcess` on the escaped text so `__MARK__…` sentinels are
  // already swapped for their HTML on the very first paint — without
  // this, every node switch flashes raw markers for the frames it
  // takes shiki to tokenise.
  if (postProcess) {
    return (
      <pre
        className={chrome}
        style={cvStyle}
        dangerouslySetInnerHTML={{ __html: postProcess(escHTML(code)) }}
      />
    );
  }
  return (
    <pre className={chrome} style={cvStyle}>
      {code}
    </pre>
  );
});
