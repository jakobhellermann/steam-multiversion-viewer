// TODO(ai-review): review for style and correctness
import { describe, expect, test } from "vitest";

import { type CodeLink, highlight, linkPlain } from "./syntax";

const CODE = 'float x = Mathf.Clamp(1f, 2f, 3f); // "Mathf"';
const at = (word: string, href: string): CodeLink => {
  const start = CODE.indexOf(word);
  return { start, end: start + word.length, attrs: { href } };
};

function anchors(html: string): [string, string][] {
  return [...html.matchAll(/<a href="([^"]*)"[^>]*>(.*?)<\/a>/g)].map((m) => [
    m[1],
    m[2].replace(/<[^>]*>/g, ""),
  ]);
}

describe("code links", () => {
  test("highlight wraps exactly the linked span, even inside a merged token", async () => {
    const html = await highlight(CODE, "csharp", [at("Mathf", "#type:UnityEngine.Mathf")]);
    expect(anchors(html)).toEqual([["#type:UnityEngine.Mathf", "Mathf"]]);
  });

  test("linkPlain escapes and shifts by the slice offset", () => {
    const text = "a < b;\nMathf.PI";
    const start = text.indexOf("Mathf");
    const link = { start, end: start + 5, attrs: { href: "/x?a=1&b=2" } };
    expect(linkPlain(text.slice(7), [link], 7)).toEqual('<a href="/x?a=1&amp;b=2">Mathf</a>.PI');
    expect(linkPlain(text.slice(0, 6), [link], 0)).toEqual("a &lt; b;");
  });
});
