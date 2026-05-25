// TODO(ai-review): review for style and correctness
import { useQuery } from "@tanstack/react-query";
import { memo } from "react";

import { fetchTransformedFile, type FileView } from "../../api";
import { formatBytes } from "../../lib/format";
import { mediaKindForPath } from "../../lib/mediaKind";
import { highlight, langForMime, langForPath } from "../../lib/syntax";
import { MediaPlayer } from "./MediaPlayer";
import { StructuredView } from "./StructuredView";
import type { FileLocator } from "./types";

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
  if (media === "image") {
    return (
      <section className={sectionClass}>
        {header}
        <img
          src={rawSrc}
          alt={view.path}
          className="max-w-full rounded border border-slate-800 bg-slate-950"
        />
      </section>
    );
  } else if (media === "audio") {
    return (
      <section className={sectionClass}>
        {header}
        <MediaPlayer kind="audio" src={rawSrc} />
      </section>
    );
  } else if (media === "video") {
    return (
      <section className={sectionClass}>
        {header}
        <MediaPlayer kind="video" src={rawSrc} />
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
}: {
  code: string;
  lang: ReturnType<typeof langForPath>;
  /// When true, skip the rounded border + padding chrome — useful when
  /// the caller already wraps the content in a styled card so we don't
  /// nest borders.
  bare?: boolean;
}) {
  const html = useQuery({
    queryKey: ["syntax-highlight", lang, code.length, code.slice(0, 64)],
    queryFn: () => (lang ? highlight(code, lang) : Promise.resolve(null)),
    enabled: lang != null,
    staleTime: Infinity,
    gcTime: Infinity,
  });
  // Shiki emits its own <pre> with the theme background; wrap so our
  // own padding/border/scroll behavior stays consistent.
  if (html.data) {
    const chrome = bare
      ? "text-xs [&_pre]:m-0! [&_pre]:bg-transparent! [&_pre]:p-0!"
      : "overflow-x-auto rounded border border-slate-800 text-xs [&_pre]:m-0! [&_pre]:bg-slate-950! [&_pre]:p-3!";
    return <div className={chrome} dangerouslySetInnerHTML={{ __html: html.data }} />;
  }
  const chrome = bare
    ? "font-mono text-xs wrap-break-word whitespace-pre-wrap"
    : "overflow-x-auto rounded border border-slate-800 bg-slate-950 p-3 font-mono text-xs wrap-break-word whitespace-pre-wrap";
  return <pre className={chrome}>{code}</pre>;
});
