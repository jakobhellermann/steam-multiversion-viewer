// TODO(ai-review): review for style and correctness
const BYTE_UNITS = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];

/**
 * Format a byte count with IEC binary units, one decimal place.
 *
 * Promotes to the next unit slightly before 1024 so values like 1021.4 MiB
 * round to 1.0 GiB instead of staying as a four-digit fraction in the smaller
 * unit. Threshold is 1000 — covers most cosmetic cases without making small
 * round numbers look weird.
 */
export function formatBytes(n: number): string {
  let v = n;
  let i = 0;
  while (v >= 1000 && i < BYTE_UNITS.length - 1) {
    v /= 1024;
    i++;
  }
  return i === 0 ? `${n} ${BYTE_UNITS[0]}` : `${v.toFixed(1)} ${BYTE_UNITS[i]}`;
}

/**
 * Like `formatBytes` but with three decimal places — for tooltips where the
 * reader wants more precision than the compact one-decimal display.
 */
export function formatBytesPrecise(n: number): string {
  let v = n;
  let i = 0;
  while (v >= 1000 && i < BYTE_UNITS.length - 1) {
    v /= 1024;
    i++;
  }
  return i === 0 ? `${n} ${BYTE_UNITS[0]}` : `${v.toFixed(3)} ${BYTE_UNITS[i]}`;
}

const dateFormatter = new Intl.DateTimeFormat(undefined, { dateStyle: "medium" });

/// Locale-aware short date for unix-second timestamps. Returns "—" for zero.
export function formatDate(unix: number): string {
  if (!unix) return "—";
  return dateFormatter.format(new Date(unix * 1000));
}
