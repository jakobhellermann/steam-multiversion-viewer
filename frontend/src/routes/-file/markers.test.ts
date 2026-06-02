// TODO(ai-review): review for style and correctness
import { describe, expect, test } from "vitest";

import { makeDiffPostProcess } from "./markers";
import type { FileLocator } from "./types";

const BASE_MANIFEST = "708613018541602983";
const TARGET_MANIFEST = "5850920340624324409";

const base: FileLocator = {
  appid: 367520,
  depotId: 367523,
  manifestId: BASE_MANIFEST,
  branch: "public",
  path: "hollow_knight_Data/level146",
};
const target: FileLocator = { ...base, manifestId: TARGET_MANIFEST };

const SEP = "␞";
const REF_FILE = "hollow_knight_Data/sharedassets0.assets";
const encFile = encodeURIComponent(REF_FILE);
// A pptr marker as it appears in the raw dump: a JSON string value.
const marker = (pathId: number) =>
  `"__MARK__pptr${SEP}obj:${pathId}${SEP}CommonSettings${SEP}LightingSettings${SEP}${REF_FILE}"`;

describe("makeDiffPostProcess", () => {
  test("removed line links into target manifest, added line into base", () => {
    const diff = [
      `--- 367523/${TARGET_MANIFEST}  2026-01-26`,
      `+++ 367523/${BASE_MANIFEST}   2026-03-27`,
      "@@ -16,7 +16,7 @@",
      `-  "m_LightingSettings": ${marker(176)},`,
      `+  "m_LightingSettings": ${marker(183)},`,
      "   }",
    ].join("\n");

    const lines = makeDiffPostProcess(base, target, () => true)(diff).split("\n");
    const removed = lines[3];
    const added = lines[4];

    // Removed (-) side: the old manifest, with its own PathID.
    expect(removed).toContain(`/manifests/${TARGET_MANIFEST}/file?path=${encFile}#obj:176`);
    expect(removed).not.toContain(BASE_MANIFEST);

    // Added (+) side: the base manifest, with its PathID.
    expect(added).toContain(`/manifests/${BASE_MANIFEST}/file?path=${encFile}#obj:183`);
    expect(added).not.toContain(TARGET_MANIFEST);
  });

  test("recovers the diff side through shiki line markup", () => {
    // Shiki wraps each line in spans and may color the gutter token; the
    // side detection must see through that markup.
    const shikiLine = (gutter: string, body: string) =>
      `<span class="line"><span style="color:#569cd6">${gutter}</span><span style="color:#ce9178">${body}</span></span>`;
    const html = [
      shikiLine("-", `  "m_LightingSettings": ${marker(176)},`),
      shikiLine("+", `  "m_LightingSettings": ${marker(183)},`),
    ].join("\n");

    const lines = makeDiffPostProcess(base, target, () => true)(html).split("\n");
    expect(lines[0]).toContain(`/manifests/${TARGET_MANIFEST}/file?path=${encFile}#obj:176`);
    expect(lines[1]).toContain(`/manifests/${BASE_MANIFEST}/file?path=${encFile}#obj:183`);
  });

  test("context line (unchanged ref) resolves against the base manifest", () => {
    const diff = [`   "m_LightingSettings": ${marker(183)},`].join("\n");
    const out = makeDiffPostProcess(base, target, () => true)(diff);
    expect(out).toContain(`/manifests/${BASE_MANIFEST}/file?path=${encFile}#obj:183`);
    expect(out).not.toContain(TARGET_MANIFEST);
  });
});
