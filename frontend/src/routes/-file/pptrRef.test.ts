// TODO(ai-review): review for style and correctness
import { describe, expect, test } from "vitest";

import { parsePptrRef, pptrNodeKeys, projectRefToSide, qualifyRefForSide } from "./pptrRef";

describe("negative path ids", () => {
  const p = "archive:CAB-bf54a70ab04d641cdc3c945b2a30ad8d/";
  test("pptrNodeKeys splits a mod pair with negative ids", () => {
    expect(pptrNodeKeys(`${p}mod:obj:-9078353595027427756,obj:-3874018262522925614`)).toEqual([
      { side: "base", key: `${p}obj:-9078353595027427756` },
      { side: "target", key: `${p}obj:-3874018262522925614` },
    ]);
  });
  test("parsePptrRef handles negative ids", () => {
    expect(parsePptrRef("obj:-42")).toEqual({ side: "either", key: "obj:-42" });
    expect(parsePptrRef("base:obj:-42")).toEqual({ side: "base", key: "obj:-42" });
  });
  test("project + qualify work for a negative mod pair", () => {
    const ref = `${p}mod:obj:-9078353595027427756,obj:-3874018262522925614`;
    expect(projectRefToSide(ref, "base")).toBe(`${p}obj:-9078353595027427756`);
    expect(qualifyRefForSide(ref, "target")).toBe(`${p}target:obj:-3874018262522925614`);
  });
});

describe("projectRefToSide", () => {
  test("matched pair answers to either side (bare + archive-prefixed)", () => {
    expect(projectRefToSide("obj:4283", "base")).toBe("obj:4283");
    expect(projectRefToSide("obj:4283", "target")).toBe("obj:4283");
    const p = "archive:CAB-59dabf0b3c64c0a529eaeb28cb24e5b8/";
    expect(projectRefToSide(`${p}obj:4283`, "base")).toBe(`${p}obj:4283`);
    expect(projectRefToSide(`${p}obj:4283`, "target")).toBe(`${p}obj:4283`);
  });

  test("renumbered pair hands back the requested side's id", () => {
    expect(projectRefToSide("mod:obj:1478,obj:1581", "base")).toBe("obj:1478");
    expect(projectRefToSide("mod:obj:1478,obj:1581", "target")).toBe("obj:1581");
  });

  test("one-sided row resolves only on its own side", () => {
    expect(projectRefToSide("base:obj:2193", "base")).toBe("obj:2193");
    expect(projectRefToSide("base:obj:2193", "target")).toBeUndefined();
    expect(projectRefToSide("target:obj:4367", "target")).toBe("obj:4367");
    expect(projectRefToSide("target:obj:4367", "base")).toBeUndefined();
  });

  test("bare rows project to both sides — any selection carries", () => {
    expect(projectRefToSide("section:hierarchy", "base")).toBe("section:hierarchy");
    expect(projectRefToSide("file:level4", "target")).toBe("file:level4");
  });

  test("addressables key ids project like any other format", () => {
    const mod = "mod:key:textures_a_4c50.bundle,key:textures_a_934d.bundle";
    expect(projectRefToSide(mod, "base")).toBe("key:textures_a_4c50.bundle");
    expect(projectRefToSide(mod, "target")).toBe("key:textures_a_934d.bundle");
    expect(projectRefToSide("base:key:Scenes/OnlyBase", "base")).toBe("key:Scenes/OnlyBase");
    expect(projectRefToSide("key:Scenes/Peak_07", "target")).toBe("key:Scenes/Peak_07");
  });
});

describe("qualifyRefForSide", () => {
  test("keeps the side tag so the ref pins to the retained manifest's index", () => {
    expect(qualifyRefForSide("obj:4283", "target")).toBe("target:obj:4283");
    expect(qualifyRefForSide("mod:obj:1478,obj:1581", "target")).toBe("target:obj:1581");
    expect(qualifyRefForSide("mod:obj:1478,obj:1581", "base")).toBe("base:obj:1478");
    expect(qualifyRefForSide("target:obj:4367", "target")).toBe("target:obj:4367");
  });

  test("tag goes after the archive prefix", () => {
    const p = "archive:CAB-59dabf0b3c64c0a529eaeb28cb24e5b8/";
    expect(qualifyRefForSide(`${p}obj:4283`, "target")).toBe(`${p}target:obj:4283`);
    expect(qualifyRefForSide(`${p}mod:obj:24,obj:13`, "base")).toBe(`${p}base:obj:24`);
  });

  test("no counterpart on the requested side → undefined (no wrong hash)", () => {
    expect(qualifyRefForSide("base:obj:2193", "target")).toBeUndefined();
    expect(qualifyRefForSide("target:obj:4367", "base")).toBeUndefined();
  });
});
