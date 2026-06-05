// TODO(ai-review): review for style and correctness
import { describe, expect, test } from "vitest";

import { makeDiffPostProcess, makePostProcess } from "./markers";
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

describe("makePostProcess local-ref side tagging", () => {
  // A same-file (local) pptr: no file part, so it's an in-page hash jump.
  const localMarker = (pathId: number) =>
    `"__MARK__pptr${SEP}obj:${pathId}${SEP}PathID=${pathId}${SEP}Transform${SEP}"`;

  test("one-sided removed body tags refs target", () => {
    const out = makePostProcess(target, () => true, "target")(`  "component": ${localMarker(42)}`);
    expect(out).toContain('href="#target:obj:42"');
    expect(out).toContain('data-pptr-ref="target:obj:42"');
  });

  test("one-sided added body tags refs base", () => {
    const out = makePostProcess(base, () => true, "base")(`  "component": ${localMarker(42)}`);
    expect(out).toContain('href="#base:obj:42"');
  });

  test("single-file view (no side) leaves refs bare", () => {
    const out = makePostProcess(base, () => true)(`  "component": ${localMarker(42)}`);
    expect(out).toContain('href="#obj:42"');
    expect(out).not.toContain("base:obj:42");
  });
});

describe("makePostProcess classref", () => {
  const DLL = "hollow_knight_Data/Managed/Assembly-CSharp.dll";
  const classrefMarker = (fqn: string) =>
    `"__MARK__classref${SEP}type:${fqn}${SEP}${fqn}${SEP}${DLL}"`;

  test("m_ClassName links into the managed dll at the type hash", () => {
    const html = `  "m_ClassName": ${classrefMarker("SceneManager")},`;
    const out = makePostProcess(base, () => true)(html);
    expect(out).toContain(
      `/manifests/${BASE_MANIFEST}/file?path=${encodeURIComponent(DLL)}#type:SceneManager`,
    );
    // The on-screen label is the class name, and there's no pptr-style
    // `(type)` suffix on a classref.
    expect(out).toContain(">SceneManager</a>");
    expect(out).not.toContain("(MonoScript)");
  });

  test("namespaced class keeps the dotted FQN in both label and hash", () => {
    const html = `  "m_ClassName": ${classrefMarker("Foo.Bar.SceneManager")},`;
    const out = makePostProcess(base, () => true)(html);
    expect(out).toContain("#type:Foo.Bar.SceneManager");
    expect(out).toContain(">Foo.Bar.SceneManager</a>");
  });
});

describe("makePostProcess shape", () => {
  test("renders one polygon per shape with a count tooltip", () => {
    const html = `  "m_PhysicsShape": "__MARK__shape${SEP}0,0;1,0;0,1|2,2;3,3",`;
    const out = makePostProcess(base, () => true)(html);
    expect(out).toContain("<svg");
    expect((out.match(/<polygon/g) ?? []).length).toBe(2);
    expect(out).toContain("<title>2 polygons, 5 pts</title>");
    // The JSON comma after the value survives (the marker only replaces
    // the quoted string).
    expect(out.trimEnd().endsWith(",")).toBe(true);
  });

  test("rejects a payload with non-numeric junk", () => {
    const html = `  "x": "__MARK__shape${SEP}<script>",`;
    const out = makePostProcess(base, () => true)(html);
    expect(out).not.toContain("<svg");
    expect(out).not.toContain("<script>");
  });
});
