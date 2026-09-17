// TODO(ai-review): review for style and correctness
import { createHighlighterCore, type HighlighterCore } from "shiki/core";
import { createJavaScriptRegexEngine } from "shiki/engine/javascript";

/// Grammars we ship, as lazy imports. Single source of truth: the keys
/// give the `Lang` type, the values are loaded into the highlighter.
/// The fine-grained `shiki/core` bundle keeps rolldown from emitting a
/// chunk per bundled grammar (~200 of them) that we never load. Add to
/// this *and* to extToLang below. `playmakerfsm` is ours, not from
/// @shikijs/langs: a custom grammar for the backend's PlayMaker FSM
/// pseudocode dump.
const GRAMMARS = {
  xml: () => import("@shikijs/langs/xml"),
  json: () => import("@shikijs/langs/json"),
  lua: () => import("@shikijs/langs/lua"),
  csharp: () => import("@shikijs/langs/csharp"),
  diff: () => import("@shikijs/langs/diff"),
  glsl: () => import("@shikijs/langs/glsl"),
  cpp: () => import("@shikijs/langs/cpp"),
  playmakerfsm: () => import("./playmakerfsmGrammar"),
} as const;
export type Lang = keyof typeof GRAMMARS;

const THEME = "github-dark";

let highlighterPromise: Promise<HighlighterCore> | null = null;

/// Lazily build a single shared Highlighter — shiki recommends one
/// instance per app, and our grammars together weigh a few hundred KB
/// we don't want to ship on first paint.
export function getHighlighter(): Promise<HighlighterCore> {
  if (!highlighterPromise) {
    highlighterPromise = createHighlighterCore({
      themes: [import("@shikijs/themes/github-dark")],
      langs: Object.values(GRAMMARS).map((load) => load()),
      // JS regex engine over oniguruma-wasm: drops the ~600KB wasm chunk.
      // Our grammars translate cleanly; a future grammar the JS engine can't
      // express throws here loudly rather than mis-highlighting in silence.
      engine: createJavaScriptRegexEngine(),
    });
  }
  return highlighterPromise;
}

const EXT_TO_LANG: Record<string, Lang> = {
  xml: "xml",
  xaml: "xml",
  svg: "xml",
  config: "xml",
  json: "json",
  jsonc: "json",
  lua: "lua",
  cs: "csharp",
};

export function langForPath(path: string): Lang | null {
  const slash = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
  const name = slash >= 0 ? path.slice(slash + 1) : path;
  const dot = name.lastIndexOf(".");
  if (dot < 0) return null;
  const ext = name.slice(dot + 1).toLowerCase();
  return EXT_TO_LANG[ext] ?? null;
}

const MIME_TO_LANG: Record<string, Lang> = {
  "text/x-diff": "diff",
  "text/x-csharp": "csharp",
  "application/x-csharp": "csharp",
  "application/json": "json",
  "text/xml": "xml",
  "application/xml": "xml",
  "text/x-lua": "lua",
  "text/x-glsl": "glsl",
  "text/x-metal": "cpp",
  "text/x-playmaker-fsm": "playmakerfsm",
};

export function langForMime(mime: string): Lang | null {
  return MIME_TO_LANG[mime.toLowerCase()] ?? null;
}

/// Render `code` as syntax-highlighted HTML. Returns the HTML string of
/// a `<pre>` element ready for `dangerouslySetInnerHTML`.
export async function highlight(code: string, lang: Lang): Promise<string> {
  const h = await getHighlighter();
  return h.codeToHtml(code, { lang, theme: THEME });
}
