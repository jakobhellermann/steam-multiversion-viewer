// TODO(ai-review): review for style and correctness
import { describe, expect, test } from "vitest";

import { parsePptrRef, pptrNodeKeys } from "./pptrRef";

describe("pptrNodeKeys", () => {
  test("matched node answers to either side under its bare id", () => {
    expect(pptrNodeKeys("obj:14")).toEqual([
      { side: "base", key: "obj:14" },
      { side: "target", key: "obj:14" },
    ]);
  });

  test("one-sided nodes key only on their own side", () => {
    expect(pptrNodeKeys("base:obj:14")).toEqual([{ side: "base", key: "obj:14" }]);
    expect(pptrNodeKeys("target:obj:14")).toEqual([{ side: "target", key: "obj:14" }]);
  });

  test("renumbered match keys base under its id, target under its id", () => {
    expect(pptrNodeKeys("mod:obj:24,obj:13")).toEqual([
      { side: "base", key: "obj:24" },
      { side: "target", key: "obj:13" },
    ]);
  });

  test("bundle archive prefix is preserved", () => {
    const p = "archive:CAB-7aadcd89622537db5c44460bffb75f95/";
    expect(pptrNodeKeys(`${p}base:obj:293`)).toEqual([{ side: "base", key: `${p}obj:293` }]);
    expect(pptrNodeKeys(`${p}mod:obj:24,obj:13`)).toEqual([
      { side: "base", key: `${p}obj:24` },
      { side: "target", key: `${p}obj:13` },
    ]);
  });

  test("non-object rows yield no keys", () => {
    expect(pptrNodeKeys("file:level4")).toEqual([]);
    expect(pptrNodeKeys("section:hierarchy")).toEqual([]);
    expect(pptrNodeKeys("archive:CAB-abc/section:loose")).toEqual([]);
  });
});

describe("parsePptrRef", () => {
  test("side-tagged refs carry their side", () => {
    expect(parsePptrRef("base:obj:13")).toEqual({ side: "base", key: "obj:13" });
    expect(parsePptrRef("target:obj:13")).toEqual({ side: "target", key: "obj:13" });
    expect(parsePptrRef("archive:CAB-x/base:obj:13")).toEqual({
      side: "base",
      key: "archive:CAB-x/obj:13",
    });
  });

  test("bare ref has no side hint", () => {
    expect(parsePptrRef("obj:13")).toEqual({ side: "either", key: "obj:13" });
    expect(parsePptrRef("archive:CAB-x/obj:13")).toEqual({
      side: "either",
      key: "archive:CAB-x/obj:13",
    });
  });

  test("non-object refs do not parse", () => {
    expect(parsePptrRef("section:hierarchy")).toBeNull();
  });

  // The collision this whole change exists for: path id 13 is `Lit` on
  // the base side but the target half of a renumbered match on the
  // other. The side keeps them apart.
  test("collision case resolves by side", () => {
    const base = new Map<string, string>();
    const target = new Map<string, string>();
    for (const id of ["mod:obj:24,obj:13", "base:obj:13"]) {
      for (const { side, key } of pptrNodeKeys(id)) {
        const map = side === "base" ? base : target;
        if (!map.has(key)) map.set(key, id);
      }
    }
    const baseRef = parsePptrRef("base:obj:13")!;
    const targetRef = parsePptrRef("target:obj:13")!;
    expect(base.get(baseRef.key)).toBe("base:obj:13");
    expect(target.get(targetRef.key)).toBe("mod:obj:24,obj:13");
  });
});
