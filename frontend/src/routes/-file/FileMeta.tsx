// TODO(ai-review): review for style and correctness
import type { FileView } from "../../api";
import { Bytes } from "../../components/Bytes";

/// Header line below the path: size, optional symlink-target, and (for
/// regular files) either an "on disk ✓" badge or a download button.
export function FileMeta({
  view,
  onDownload,
  downloadPending,
}: {
  view: FileView;
  onDownload: () => void;
  downloadPending: boolean;
}) {
  const fullyOnDisk = view.chunks_present === view.chunk_count;
  const missingChunks = view.chunk_count - view.chunks_present;
  return (
    <p className="mt-1 flex items-baseline gap-3 text-sm text-slate-400">
      <span>
        <Bytes value={view.size} />
      </span>
      {view.kind === "symlink" && view.linktarget && (
        <span className="text-slate-500">
          → <span className="font-mono text-slate-300">{view.linktarget}</span>
        </span>
      )}
      {view.kind === "file" &&
        view.chunk_count > 0 &&
        (fullyOnDisk ? (
          <span className="text-emerald-400">on disk ✓</span>
        ) : (
          <button
            type="button"
            onClick={onDownload}
            disabled={downloadPending}
            className="rounded border border-sky-700 bg-sky-950/40 px-2 py-0.5 text-xs hover:bg-sky-900/40 disabled:opacity-40"
          >
            {downloadPending
              ? "enqueuing…"
              : `download ${missingChunks} missing chunk${missingChunks === 1 ? "" : "s"}`}
          </button>
        ))}
    </p>
  );
}
