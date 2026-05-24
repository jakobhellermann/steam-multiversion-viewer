// TODO(ai-review): review for style and correctness
import { describe, expect, test } from "vitest";
import { formatBytes, formatBytesPrecise } from "./format";

describe("formatBytes", () => {
  test("bytes stay as integer up to threshold", () => {
    expect(formatBytes(0)).toBe("0 B");
    expect(formatBytes(1)).toBe("1 B");
    expect(formatBytes(999)).toBe("999 B");
  });

  test("promotes at 1000 even though unit boundary is 1024", () => {
    // 1013.8 MiB used to render as a 4-digit fraction; we want it as GiB.
    const oneThousand13ish_MiB = Math.round(1013.8 * 1024 * 1024);
    expect(formatBytes(oneThousand13ish_MiB)).toBe("1.0 GiB");
  });

  test("round numbers in their natural unit", () => {
    expect(formatBytes(1024)).toBe("1.0 KiB");
    expect(formatBytes(1024 * 1024)).toBe("1.0 MiB");
    expect(formatBytes(1024 ** 3)).toBe("1.0 GiB");
  });

  test("keeps unit when comfortably below threshold", () => {
    expect(formatBytes(500 * 1024 * 1024)).toBe("500.0 MiB");
    expect(formatBytes(900 * 1024)).toBe("900.0 KiB");
  });

  test("large values cap at PiB", () => {
    expect(formatBytes(1024 ** 5)).toBe("1.0 PiB");
    expect(formatBytes(10 * 1024 ** 5)).toBe("10.0 PiB");
  });
});

describe("formatBytesPrecise", () => {
  test("three decimal places in promoted unit", () => {
    expect(formatBytesPrecise(1024 ** 3)).toBe("1.000 GiB");
    expect(formatBytesPrecise(1024 * 1024 * 1024 + 38 * 1024 * 1024)).toBe("1.037 GiB");
  });

  test("raw bytes below threshold", () => {
    expect(formatBytesPrecise(500)).toBe("500 B");
  });
});
