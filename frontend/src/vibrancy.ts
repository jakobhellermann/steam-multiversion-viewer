// Windows backdrop wiring: Rust owns the window transparency + DWM effect; the
// frontend applies the darkening tint and requests the effect over ipc.

import type { Config } from "./api";

declare global {
  interface Window {
    // Injected by the native Windows window. Present => native window; `active`
    // => transparent backdrop is live this session (fixed at window creation).
    __vibrancy?: { active: boolean };
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
