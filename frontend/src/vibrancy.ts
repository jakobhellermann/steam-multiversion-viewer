// Windows backdrop wiring: Rust owns the window transparency + DWM effect; the
// frontend applies the darkening tint and requests the effect over ipc.

import type { Config } from "./api";

declare global {
  interface Window {
    // Injected by the native Windows window. Present => native window; `active`
    // => transparent backdrop is live this session (fixed at window creation).
    __vibrancy?: { active: boolean };
    // Injected by the native macOS window: enables the app-detail preview dropdown.
    __macTestVibrancy?: boolean;
    ipc?: { postMessage: (msg: string) => void };
  }
}

/** Whether we're in the native Windows window (the settings controls apply here). */
export function vibrancySupported(): boolean {
  return typeof window !== "undefined" && window.__vibrancy !== undefined;
}

/** Whether the transparent backdrop is live this session (set at window creation). */
export function vibrancyActive(): boolean {
  return typeof window !== "undefined" && window.__vibrancy?.active === true;
}

type VibrancySettings = Pick<Config, "vibrancy_effect" | "vibrancy_tint">;

/** Apply the tint (CSS) and request the effect from Rust (ipc). No-op unless the
 *  backdrop is active, so it never affects other platforms or the browser. */
export function applyVibrancy(cfg: VibrancySettings): void {
  if (!vibrancyActive()) return;
  const root = document.documentElement;
  const on = cfg.vibrancy_effect !== "none";
  root.classList.toggle("vibrancy-active", on);
  const tint = Math.min(100, Math.max(0, cfg.vibrancy_tint)) / 100;
  root.style.setProperty("--vibrancy-tint", on ? `rgba(15, 23, 42, ${tint})` : "transparent");
  window.ipc?.postMessage(cfg.vibrancy_effect);
}

// --- macOS material preview (test-only, app-detail dropdown) ---------------

/** In the native macOS window, where the preview dropdown applies. */
export function macVibrancyTestSupported(): boolean {
  return typeof window !== "undefined" && window.__macTestVibrancy === true;
}

/** Groups of backdrop variants for the preview dropdown. Keys are the ipc body
 *  the Rust side maps to an NSGlassEffectView style / NSVisualEffectMaterial. */
export const MAC_VIBRANCY_VARIANTS: { group: string; items: { key: string; label: string }[] }[] = [
  {
    group: "Liquid Glass (macOS 26)",
    items: [
      { key: "glass-regular", label: "Glass · Regular" },
      { key: "glass-clear", label: "Glass · Clear" },
      { key: "glass-dock", label: "Glass · Dock (private)" },
      { key: "glass-sidebar", label: "Glass · Sidebar (private)" },
      { key: "glass-inspector", label: "Glass · Inspector (private)" },
      { key: "glass-widgets", label: "Glass · Widgets (private)" },
      { key: "glass-control", label: "Glass · Control (private)" },
      { key: "glass-loupe", label: "Glass · Loupe (private)" },
      { key: "glass-bubbles", label: "Glass · Bubbles (private)" },
    ],
  },
  {
    group: "Vibrancy materials",
    items: [
      { key: "vibrancy-under-window-background", label: "Under Window Background" },
      { key: "vibrancy-hud-window", label: "HUD Window" },
      { key: "vibrancy-sidebar", label: "Sidebar" },
      { key: "vibrancy-window-background", label: "Window Background" },
      { key: "vibrancy-under-page-background", label: "Under Page Background" },
      { key: "vibrancy-content-background", label: "Content Background" },
      { key: "vibrancy-popover", label: "Popover" },
      { key: "vibrancy-menu", label: "Menu" },
      { key: "vibrancy-header-view", label: "Header View" },
      { key: "vibrancy-sheet", label: "Sheet" },
      { key: "vibrancy-fullscreen-ui", label: "Fullscreen UI" },
      { key: "vibrancy-titlebar", label: "Titlebar" },
      { key: "vibrancy-selection", label: "Selection" },
      { key: "vibrancy-tooltip", label: "Tooltip" },
    ],
  },
];

/** Preview one variant live: make the page see-through (no tint) and ask Rust to
 *  swap the native effect. `none` restores the opaque background. */
export function applyMacVibrancyVariant(key: string): void {
  document.documentElement.classList.toggle("vibrancy-active", key !== "none");
  window.ipc?.postMessage(key);
}
