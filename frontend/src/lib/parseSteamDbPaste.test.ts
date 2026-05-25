// TODO(ai-review): review for style and correctness
import { describe, expect, test } from "vitest";
import { parseSteamDbPaste, type ParsedExtra } from "./parseSteamDbPaste";

function bare(manifest_id: string, branch = "public"): ParsedExtra {
  return { manifest_id, branch, app_id: null, depot_id: null };
}

describe("parseSteamDbPaste — SteamDB rows", () => {
  test("empty input yields no entries", () => {
    expect(parseSteamDbPaste("")).toEqual([]);
    expect(parseSteamDbPaste("   \n  \n")).toEqual([]);
  });

  test("steamdb paste with header is parsed (public + branch rows)", () => {
    const input = `Previously seen manifests
 Filter branch


- unfiltered -
Copy format:
Seen Date    Relative Date    ManifestID
24 March 2026 – 22:56:53 UTC    2 months ago    4421626056705534276
20 March 2026 – 08:31:28 UTC    2 months ago    468692862190470536
19 March 2026 – 08:51:17 UTC    2 months ago    4190027372378340804 public-beta
17 March 2026 – 00:48:53 UTC    2 months ago    3703686001292550981
13 March 2026 – 05:42:25 UTC    2 months ago    3853982342899707391
6 March 2026 – 04:05:41 UTC    2 months ago    1365728208972354992 public-beta
25 February 2026 – 05:47:21 UTC    3 months ago    7697132700953129081 public-beta
17 February 2026 – 03:49:55 UTC    3 months ago    122836693421754773 public-beta
12 November 2025 – 08:41:37 UTC    6 months ago    3545882420322545098
12 November 2025 – 00:02:03 UTC    6 months ago    7267592975921547533 public-beta
10 November 2025 – 09:15:54 UTC    6 months ago    3303638851425867673 public-beta
6 November 2025 – 08:49:44 UTC    6 months ago    426651197780377263
6 November 2025 – 05:47:19 UTC    6 months ago    1087113759402695494 public-beta
5 November 2025 – 05:37:39 UTC    6 months ago    1126896070346294988 public-beta
28 October 2025 – 07:55:23 UTC    7 months ago    4112638627367462198 public-beta
28 October 2025 – 00:50:11 UTC    7 months ago    2972586027529664984 public-beta
16 October 2025 – 03:33:26 UTC    7 months ago    2538255789859855032 public-beta
7 October 2025 – 11:50:27 UTC    7 months ago    3690203822520536668
3 October 2025 – 10:43:52 UTC    7 months ago    1192622377201554207 public-beta
3 October 2025 – 07:46:43 UTC    7 months ago    5977483240701257214
26 September 2025 – 12:03:54 UTC    8 months ago    2396501635281238168 public-beta
24 September 2025 – 09:24:55 UTC    8 months ago    6773297648406051922
17 September 2025 – 13:15:21 UTC    8 months ago    3900764848237536293
16 September 2025 – 10:55:31 UTC    8 months ago    8253685481868764204 public-beta
12 September 2025 – 12:20:45 UTC    8 months ago    8642535143474926050
11 September 2025 – 00:02:15 UTC    8 months ago    539129767115354441
9 September 2025 – 07:57:21 UTC    8 months ago    4354007652312393230 public-beta
5 September 2025 – 11:47:28 UTC    8 months ago    2291879799325603580 public-beta
4 September 2025 – 14:00:15 UTC    8 months ago    3229726349000518284    `;

    expect(parseSteamDbPaste(input)).toEqual([
      bare("4421626056705534276"),
      bare("468692862190470536"),
      bare("4190027372378340804", "public-beta"),
      bare("3703686001292550981"),
      bare("3853982342899707391"),
      bare("1365728208972354992", "public-beta"),
      bare("7697132700953129081", "public-beta"),
      bare("122836693421754773", "public-beta"),
      bare("3545882420322545098"),
      bare("7267592975921547533", "public-beta"),
      bare("3303638851425867673", "public-beta"),
      bare("426651197780377263"),
      bare("1087113759402695494", "public-beta"),
      bare("1126896070346294988", "public-beta"),
      bare("4112638627367462198", "public-beta"),
      bare("2972586027529664984", "public-beta"),
      bare("2538255789859855032", "public-beta"),
      bare("3690203822520536668"),
      bare("1192622377201554207", "public-beta"),
      bare("5977483240701257214"),
      bare("2396501635281238168", "public-beta"),
      bare("6773297648406051922"),
      bare("3900764848237536293"),
      bare("8253685481868764204", "public-beta"),
      bare("8642535143474926050"),
      bare("539129767115354441"),
      bare("4354007652312393230", "public-beta"),
      bare("2291879799325603580", "public-beta"),
      bare("3229726349000518284"),
    ]);
  });

  test("bare manifest IDs default to public", () => {
    expect(parseSteamDbPaste("4421626056705534276\n7921642076658611197")).toEqual([
      bare("4421626056705534276"),
      bare("7921642076658611197"),
    ]);
  });

  test("dedups by manifest_id, keeps first occurrence", () => {
    expect(parseSteamDbPaste("12345678901234567 public-beta\n12345678901234567")).toEqual([
      bare("12345678901234567", "public-beta"),
    ]);
  });

  test("ignores numbers that aren't manifest-id-shaped", () => {
    // 5 digits, 21 digits, dates — none should be picked up.
    expect(parseSteamDbPaste("12345\n123456789012345678901\n2026-03-24 00:48:53")).toEqual([]);
  });

  test("arbitrary custom branch names are honored", () => {
    expect(parseSteamDbPaste("1234567890123456789 my-custom-beta")).toEqual([
      bare("1234567890123456789", "my-custom-beta"),
    ]);
    expect(parseSteamDbPaste("1234567890123456789 staging_2026")).toEqual([
      bare("1234567890123456789", "staging_2026"),
    ]);
  });
});

describe("parseSteamDbPaste — depotdownloader CLI", () => {
  test("basic -app -depot -manifest", () => {
    expect(parseSteamDbPaste("-app 1030300 -depot 1030301 -manifest 4421626056705534276")).toEqual([
      {
        manifest_id: "4421626056705534276",
        branch: "public",
        app_id: 1030300,
        depot_id: 1030301,
      },
    ]);
  });

  test("with -beta branch", () => {
    expect(
      parseSteamDbPaste(
        "-app 1030300 -depot 1030301 -manifest 4190027372378340804 -beta public-beta",
      ),
    ).toEqual([
      {
        manifest_id: "4190027372378340804",
        branch: "public-beta",
        app_id: 1030300,
        depot_id: 1030301,
      },
    ]);
  });

  test("flags in different order", () => {
    expect(parseSteamDbPaste("-manifest 4421626056705534276 -depot 1030301 -app 1030300")).toEqual([
      {
        manifest_id: "4421626056705534276",
        branch: "public",
        app_id: 1030300,
        depot_id: 1030301,
      },
    ]);
  });
});

describe("parseSteamDbPaste — Steam console download_depot", () => {
  test("three positional args", () => {
    expect(parseSteamDbPaste("download_depot 1030300 1030301 468692862190470536")).toEqual([
      {
        manifest_id: "468692862190470536",
        branch: "public",
        app_id: 1030300,
        depot_id: 1030301,
      },
    ]);
  });

  test("with trailing branch token", () => {
    expect(
      parseSteamDbPaste("download_depot 1030300 1030301 4190027372378340804 public-beta"),
    ).toEqual([
      {
        manifest_id: "4190027372378340804",
        branch: "public-beta",
        app_id: 1030300,
        depot_id: 1030301,
      },
    ]);
  });
});

describe("parseSteamDbPaste — mixed input", () => {
  test("multiple formats in one paste, dedup across", () => {
    const input = [
      "-app 1030300 -depot 1030301 -manifest 4421626056705534276",
      "download_depot 1030300 1030301 468692862190470536",
      "4190027372378340804 public-beta",
      // Duplicate of the first entry — should be skipped.
      "-app 1030300 -depot 1030301 -manifest 4421626056705534276",
    ].join("\n");
    expect(parseSteamDbPaste(input)).toEqual([
      {
        manifest_id: "4421626056705534276",
        branch: "public",
        app_id: 1030300,
        depot_id: 1030301,
      },
      {
        manifest_id: "468692862190470536",
        branch: "public",
        app_id: 1030300,
        depot_id: 1030301,
      },
      bare("4190027372378340804", "public-beta"),
    ]);
  });
});
