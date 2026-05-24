// TODO(ai-review): review for style and correctness
import { formatBytes, formatBytesPrecise } from "./format";

/// Render a byte count with the compact IEC formatting, more precise value as tooltip.
export function Bytes({ value }: { value: number }) {
  return <span title={formatBytesPrecise(value)}>{formatBytes(value)}</span>;
}
