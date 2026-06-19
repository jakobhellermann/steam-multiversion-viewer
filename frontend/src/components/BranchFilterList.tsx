// TODO(ai-review): review for style and correctness
import { Focus } from "lucide-react";
import { useEffect, useRef } from "react";

/// Presentational branch picker: a master "All" checkbox (with an
/// indeterminate state for partial selection) plus one row per branch
/// with a checkbox and a "show only this" (solo) action. Stateless —
/// the parent owns selection state and persistence, and supplies its own
/// trigger button + popover container.
export function BranchFilterList({
  branches,
  hidden,
  onToggle,
  onShowAll,
  onHideAll,
  onOnly,
}: {
  branches: string[];
  /// Branch names currently hidden. Empty = all shown.
  hidden: Set<string>;
  onToggle: (name: string) => void;
  onShowAll: () => void;
  onHideAll: () => void;
  onOnly: (name: string) => void;
}) {
  const allShown = hidden.size === 0;
  // The DOM `indeterminate` flag can only be set imperatively.
  const masterRef = useRef<HTMLInputElement>(null);
  useEffect(() => {
    if (masterRef.current) {
      masterRef.current.indeterminate = !allShown && hidden.size < branches.length;
    }
  }, [allShown, hidden.size, branches.length]);
  return (
    <>
      <div className="flex shrink-0 items-center justify-between border-b border-slate-800 px-3 py-1.5 text-xs text-slate-400">
        <label className="flex cursor-pointer items-center gap-2 select-none">
          <input
            ref={masterRef}
            type="checkbox"
            checked={allShown}
            onChange={() => (allShown ? onHideAll() : onShowAll())}
            className="accent-sky-500"
          />
          <span>All</span>
        </label>
        <span className="tabular-nums">{branches.length} branches</span>
      </div>
      <ul className="min-h-0 flex-1 overflow-y-auto py-1 text-sm">
        {branches.map((name) => {
          const checked = !hidden.has(name);
          return (
            <li key={name} className="flex items-center hover:bg-slate-800/60">
              <label className="flex flex-1 cursor-pointer items-center gap-2 px-3 py-1 select-none">
                <input
                  type="checkbox"
                  checked={checked}
                  onChange={() => onToggle(name)}
                  className="accent-sky-500"
                />
                <span className={checked ? "" : "text-slate-500"}>{name}</span>
              </label>
              <button
                type="button"
                onClick={() => onOnly(name)}
                title={`Show only ${name}`}
                aria-label={`Show only ${name}`}
                className="px-3 py-1 text-slate-500 hover:text-sky-400"
              >
                <Focus className="h-3.5 w-3.5" />
              </button>
            </li>
          );
        })}
      </ul>
    </>
  );
}
