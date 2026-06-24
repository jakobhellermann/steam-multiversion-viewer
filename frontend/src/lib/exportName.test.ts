import { describe, expect, it } from "vitest";
import { exportFolderName, joinExportPath } from "./exportName";
import type { GameInfo } from "../api";

const unity = (bundle_version?: string): GameInfo => ({
  engine: { engine: "unity", data: { version: "2020.2.2f1", bundle_version } },
});

describe("exportFolderName", () => {
  it("uses the game version when detected", () => {
    expect(exportFolderName("Hollow Knight", 367520, unity("1.5.78.11833"), "5829533265")).toBe(
      "Hollow Knight-1.5.78.11833",
    );
  });

  it("falls back to the manifest id without a version", () => {
    expect(exportFolderName("Hollow Knight", 367520, unity(undefined), "5829533265")).toBe(
      "Hollow Knight-5829533265",
    );
    expect(exportFolderName("Hollow Knight", 367520, { engine: null }, "5829533265")).toBe(
      "Hollow Knight-5829533265",
    );
    expect(exportFolderName("Hollow Knight", 367520, undefined, "5829533265")).toBe(
      "Hollow Knight-5829533265",
    );
  });

  it("falls back to the appid without a name", () => {
    expect(exportFolderName(undefined, 367520, unity("1.0"), "5829533265")).toBe("App 367520-1.0");
    expect(exportFolderName("  ", 367520, unity("1.0"), "5829533265")).toBe("App 367520-1.0");
  });

  it("folds path separators into the name", () => {
    expect(exportFolderName("Marvel/DC", 1, unity("1.0"), "5")).toBe("Marvel-DC-1.0");
    expect(exportFolderName("A\\B", 1, unity("1.0"), "5")).toBe("A-B-1.0");
  });

  it("strips characters Windows rejects but keeps spaces", () => {
    expect(exportFolderName("Hollow Knight: Silksong", 1, unity("1.0.5"), "5")).toBe(
      "Hollow Knight-Silksong-1.0.5",
    );
    expect(exportFolderName('a<b>c"d|e?f*g', 1, unity("1.0"), "5")).toBe("a-b-c-d-e-f-g-1.0");
    expect(exportFolderName("tab\there", 1, unity("1.0"), "5")).toBe("tab-here-1.0");
  });

  it("drops trailing dots and spaces", () => {
    expect(exportFolderName("Game", 1, unity("1.0."), "5")).toBe("Game-1.0");
    expect(exportFolderName("Game", 1, undefined, "5 ")).toBe("Game-5");
  });

  it("never yields an empty name", () => {
    expect(exportFolderName("/", 1, unity("/"), "5")).toBe("export");
  });
});

describe("joinExportPath", () => {
  it("follows the separator style of the root", () => {
    expect(joinExportPath("/home/alice/Downloads", "Hollow Knight-1.0")).toBe(
      "/home/alice/Downloads/Hollow Knight-1.0",
    );
    expect(joinExportPath("C:\\Users\\alice\\Downloads", "Hollow Knight-1.0")).toBe(
      "C:\\Users\\alice\\Downloads\\Hollow Knight-1.0",
    );
  });

  it("does not double the separator", () => {
    expect(joinExportPath("/home/alice/Downloads/", "A-1.0")).toBe("/home/alice/Downloads/A-1.0");
    expect(joinExportPath("C:\\Downloads\\", "A-1.0")).toBe("C:\\Downloads\\A-1.0");
  });
});
