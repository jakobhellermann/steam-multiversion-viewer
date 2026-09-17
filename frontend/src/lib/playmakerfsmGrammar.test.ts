// TODO(ai-review): review for style and correctness
import { describe, expect, test } from "vitest";

import { highlight, langForMime } from "./syntax";

/// github-dark colors, in the casing shiki emits them (upper-case hex).
const KEYWORD = "#F97583"; // keywords, operators
const ENTITY = "#B392F0"; // action names, FSM name, property members
const STATE = "#85E89D"; // state names
const CONSTANT = "#79B8FF"; // events, numbers, bools, Self
const PARAM = "#FFAB70"; // param labels, variable names
const STRING = "#9ECBFF"; // quoted strings
const COMMENT = "#6A737D"; // comments, (file) suffixes
const INVALID = "#FDAEB7"; // undecodable residue

/// The crate's documented layout, extended with one action holding the
/// constructs it leaves out (object refs, template control, enum, layer).
const SAMPLE = [
  "// uses template: bell_shrine",
  "fsm Bell Shrine {",
  "  start Init",
  "  on RESET → Init  // from any state",
  "",
  "  state Init {",
  '    SetBoolValue(boolVariable=var "On", boolValue=true, finishEvent=(none), target=Self)  // arm the bell',
  "    on FINISHED → Idle",
  "  }",
  "",
  "  sequence state Idle {",
  "    SendEvent(value=(3B))  // disabled",
  "    <deleted action>()",
  "    SetFsmBool(",
  "      gameObject=Bell Shrine/Path (sharedassets1.assets),",
  "      layer=Default(0), mode=SomeEnum(2), invoke=PlaySound(<float>),",
  "      target=loose:42, offset=(1, 0.5, 0), missing=<null>)",
  '    RunTemplate(template=bell_shrine in[Bell <- var "On"] out[Done -> FINISHED] vars[Ticks = 3] events[STARTED->DONE])',
  '    MoveTowards(property=Transform.position on var "Target")',
  "  }",
  "",
  "  var On: bool = false",
  "",
  "  var Ticks: int = 3",
  "}",
].join("\n");

/// Assert `text` got the given color. vscode-textmate glues the
/// whitespace ahead of a match onto the matched token, so tolerate
/// leading span content.
const colored = (color: string, text: string) => {
  const escaped = text.replace(/[.*+?^${}()|[\]\\-]/g, "\\$&");
  return new RegExp(`color:${color}[^>]*>[^<]*${escaped}<`);
};

describe("playmakerfsm grammar", () => {
  test("maps the MIME to the language", () => {
    expect(langForMime("text/x-playmaker-fsm")).toBe("playmakerfsm");
  });

  test("the JS regex engine accepts every pattern", async () => {
    // A pattern oniguruma-to-es can't translate throws at highlighter
    // construction, so one successful render covers all rules.
    await expect(highlight(SAMPLE, "playmakerfsm")).resolves.toBeTypeOf("string");
  });

  test("colors the structural vocabulary", async () => {
    const html = await highlight(SAMPLE, "playmakerfsm");
    expect(html).toMatch(colored(KEYWORD, "fsm"));
    expect(html).toMatch(colored(KEYWORD, "start"));
    expect(html).toMatch(colored(KEYWORD, "state"));
    expect(html).toMatch(colored(KEYWORD, "on"));
    expect(html).toMatch(colored(KEYWORD, "var"));
    expect(html).toMatch(colored(KEYWORD, "→"));
    expect(html).toMatch(colored(KEYWORD, "bool")); // storage.type
  });

  test("names: states green, actions and members purple, FSM name typed", async () => {
    const html = await highlight(SAMPLE, "playmakerfsm");
    expect(html).toMatch(colored(ENTITY, "Bell Shrine"));
    expect(html).toMatch(colored(ENTITY, "SetBoolValue"));
    expect(html).toMatch(colored(ENTITY, "RunTemplate"));
    expect(html).toMatch(colored(ENTITY, "Transform.position"));
    expect(html).toMatch(colored(STATE, "Init"));
    expect(html).toMatch(colored(STATE, "Idle"));
  });

  test("values: events and constants blue, strings light blue, labels orange", async () => {
    const html = await highlight(SAMPLE, "playmakerfsm");
    expect(html).toMatch(colored(CONSTANT, "RESET"));
    expect(html).toMatch(colored(CONSTANT, "true"));
    expect(html).toMatch(colored(CONSTANT, "(none)"));
    expect(html).toMatch(colored(CONSTANT, "Self"));
    expect(html).toMatch(colored(CONSTANT, "(1, 0.5, 0)"));
    expect(html).toMatch(colored(CONSTANT, "Default")); // layer
    expect(html).toMatch(colored(STRING, '"On"'));
    expect(html).toMatch(colored(PARAM, "boolVariable"));
    expect(html).toMatch(colored(PARAM, "Bell")); // template binding
  });

  test("comments and file suffixes are gray", async () => {
    const html = await highlight(SAMPLE, "playmakerfsm");
    expect(html).toMatch(colored(COMMENT, "// arm the bell"));
    expect(html).toMatch(colored(COMMENT, "// from any state"));
    expect(html).toMatch(colored(COMMENT, "(sharedassets1.assets)"));
  });

  test("undecodable residue is flagged invalid", async () => {
    const html = await highlight(SAMPLE, "playmakerfsm");
    expect(html).toMatch(colored(INVALID, "(3B)"));
    // shiki HTML-escapes `<` as `&#x3C;`.
    expect(html).toMatch(colored(INVALID, "&#x3C;deleted action>"));
    expect(html).toMatch(colored(INVALID, "loose:42"));
  });
});
