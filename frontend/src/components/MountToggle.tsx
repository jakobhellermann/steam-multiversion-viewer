// TODO(ai-review): review for style and correctness
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useRef } from "react";
import { fetchMountStatus, startMount, stopMount } from "../api";

/// Header icon-button that toggles the FUSE mount. Sky-blue when the
/// mount is up, slate when idle; hover-title shows the current
/// mountpoint or "Mount (idle)".
export function MountToggle() {
  const qc = useQueryClient();
  const status = useQuery({ queryKey: ["mount-status"], queryFn: fetchMountStatus });

  const mutation = useMutation({
    mutationFn: async () => {
      if (status.data?.state === "mounted") return stopMount();
      return startMount();
    },
    onSuccess: (data) => qc.setQueryData(["mount-status"], data),
    // On error the backend state may be out of sync with what we
    // assumed; refetch so the next click does the right thing.
    onError: () => qc.invalidateQueries({ queryKey: ["mount-status"] }),
  });

  const mounted = status.data?.state === "mounted";
  const title = mutation.error
    ? `Mount: ${(mutation.error as Error).message}`
    : mounted
      ? `Mounted at ${status.data?.state === "mounted" ? status.data.mountpoint : ""}`
      : "Mount (idle) — click to mount the depot tree";

  // Only mutation errors (start/stop attempts) become the "Mount failed"
  // popover. status.error usually just means the backend is down/booting
  // — render that as a disabled button via status.data === undefined.
  const lastError = mutation.error as Error | null;

  const popoverRef = useRef<HTMLDivElement | null>(null);
  useEffect(() => {
    if (!lastError) return;
    function onDown(e: MouseEvent) {
      if (!(e.target instanceof Node)) return;
      if (popoverRef.current?.contains(e.target)) return;
      mutation.reset();
    }
    // Defer attaching by a tick so the click that opened the popover
    // doesn't immediately dismiss it.
    const id = window.setTimeout(() => window.addEventListener("mousedown", onDown), 0);
    return () => {
      window.clearTimeout(id);
      window.removeEventListener("mousedown", onDown);
    };
  }, [lastError, mutation]);

  return (
    <div className="relative">
      <button
        type="button"
        onClick={() => mutation.mutate()}
        // status.data drives the start-vs-stop decision; disable until
        // we know which one to send so we don't blindly call start on a
        // backend that's already mounted (from a previous session).
        disabled={mutation.isPending || status.isPending || status.data === undefined}
        className={`relative p-1 rounded ${
          mounted ? "text-sky-400 hover:text-sky-300" : "text-slate-400 hover:text-sky-400"
        } disabled:opacity-50`}
        aria-label={mounted ? "Unmount" : "Mount"}
        title={title}
      >
        {/* Folder-tree icon — same stroke style as the gear next to it. */}
        <svg
          xmlns="http://www.w3.org/2000/svg"
          width="20"
          height="20"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          strokeWidth="2"
          strokeLinecap="round"
          strokeLinejoin="round"
        >
          <path d="M20 10a1 1 0 0 0 1-1V6a1 1 0 0 0-1-1h-2.5l-1-1.5a1 1 0 0 0-.8-.5H12a1 1 0 0 0-1 1v5a1 1 0 0 0 1 1z" />
          <path d="M20 21a1 1 0 0 0 1-1v-3a1 1 0 0 0-1-1h-7a1 1 0 0 0-1 1v3a1 1 0 0 0 1 1z" />
          <path d="M3 5v14a2 2 0 0 0 2 2h3" />
          <path d="M3 12h5" />
        </svg>
        {mounted && (
          // Top-right "live" dot. The outer ring matches the header bg
          // so the dot reads as an inset badge instead of floating.
          <span
            aria-hidden
            className="absolute top-0 right-0 w-2 h-2 rounded-full bg-emerald-400 ring-2 ring-slate-900"
          />
        )}
      </button>
      {lastError && (
        // Anchored to the icon so it doesn't push header items around.
        // Dismissed by clicking outside (see effect above).
        <div
          ref={popoverRef}
          role="alert"
          className="absolute right-0 top-full mt-2 z-50 w-96 px-4 py-3 rounded-md border border-red-700 bg-red-950/95 text-red-100 text-sm shadow-xl space-y-1"
        >
          <div className="font-semibold text-red-300">Mount failed</div>
          <div className="font-mono whitespace-pre-wrap break-words leading-snug">
            {lastError.message}
          </div>
        </div>
      )}
    </div>
  );
}
