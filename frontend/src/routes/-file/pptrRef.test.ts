// TODO(ai-review): review for style and correctness
import { describe, expect, test } from "vitest";

import { projectRefToSide, qualifyRefForSide } from "./pptrRef";

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

  test("non-object rows have no projection", () => {
    expect(projectRefToSide("section:hierarchy", "base")).toBeUndefined();
    expect(projectRefToSide("file:level4", "target")).toBeUndefined();
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
    expect(qualifyRefForSide("section:hierarchy", "target")).toBeUndefined();
  });
});
