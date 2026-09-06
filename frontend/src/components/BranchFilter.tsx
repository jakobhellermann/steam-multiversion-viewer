// TODO(ai-review): review for style and correctness
import { useEffect, useRef, useState } from "react";

import { BranchFilterList } from "./BranchFilterList";

/// Dropdown button + panel controlling branch visibility. Shared between
/// the app overview's depot section (`size="sm"`, next to a section
/// heading) and sub-page toolbars (`size="md"`, matching the boxed
/// dropdown-trigger controls on the manifest page). Fully controlled:
/// `hidden` and the toggle callbacks come from `useBranchFilter`; only
/// the open state is local.
export function BranchFilter({
  branches,
  hidden,
  onToggle,
  onShowAll,
  onHideAll,
  onOnly,
  size = "sm",
}: {
  branches: string[];
  hidden: Set<string>;
  onToggle: (name: string) => void;
  onShowAll: () => void;
  onHideAll: () => void;
  onOnly: (name: string) => void;
  size?: "sm" | "md";
}) {
  const [open, setOpen] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);
  // Close on outside click / Escape — same pattern as CompareMenu.
  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      if (rootRef.current && !rootRef.current.contains(e.target as Node)) setOpen(false);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setOpen(false);
    };
    document.addEventListener("mousedown", onDown);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onDown);
      document.removeEventListener("keydown", onKey);
    };
  }, [open]);
  const visibleCount = branches.length - hidden.size;
  // One string in one color, counting like the Compare menu does:
  // "Branches" when everything is shown, "Branches (1 of 2)" when not.
  const label = `Branches${hidden.size > 0 ? ` (${visibleCount} of ${branches.length})` : ""}`;
  return (
    <div ref={rootRef} className="relative inline-block">
      <button
        type="button"
        onClick={() => setOpen((o) => !o)}
        aria-expanded={open}
        className={
          size === "md"
            ? "flex items-center gap-1.5 dropdown-trigger border-slate-700 text-slate-300 hover:border-slate-600"
            : "flex items-center gap-1.5 rounded border border-slate-700 px-2 py-1 text-xs text-slate-300 hover:border-slate-600"
        }
      >
        <span>{label}</span>
        <span className="text-slate-500">{open ? "▲" : "▼"}</span>
      </button>
      {open && (
        <div className="dropdown-panel right-0 z-10 flex max-h-96 w-64 flex-col overflow-hidden surface-float">
          <BranchFilterList
            branches={branches}
            hidden={hidden}
            onToggle={onToggle}
            onShowAll={onShowAll}
            onHideAll={onHideAll}
            onOnly={onOnly}
          />
        </div>
      )}
    </div>
  );
}
