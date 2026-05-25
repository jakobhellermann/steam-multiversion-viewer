// TODO(ai-review): review for style and correctness

/// Tiny pub/sub flag for "show the downloads drawer immediately on the
/// next active stats burst". Default behavior is to delay the drawer by
/// ~2s so a quick file open that finishes fast never flashes the UI.
/// "Download all" buttons set this before kicking off the mutation so
/// the drawer pops up right away as the user expects.
///
/// The flag is consumed (cleared) the first time the drawer goes
/// active. It also auto-expires after SIGNAL_TTL_MS in case the click
/// produced no work at all (e.g. everything was already cached) — we
/// don't want that stale flag to short-circuit the delay on the next
/// implicit fetch the user does.

const SIGNAL_TTL_MS = 5000;

let showImmediatelyUntil = 0;
const listeners = new Set<() => void>();

export function markShowImmediately(): void {
  showImmediatelyUntil = performance.now() + SIGNAL_TTL_MS;
  for (const l of listeners) l();
}

export function consumeShowImmediately(): boolean {
  const v = performance.now() < showImmediatelyUntil;
  showImmediatelyUntil = 0;
  return v;
}

export function subscribeShowImmediately(cb: () => void): () => void {
  listeners.add(cb);
  return () => listeners.delete(cb);
}
