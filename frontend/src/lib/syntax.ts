// TODO(ai-review): review for style and correctness
import { createHighlighterCore, type HighlighterCore } from "shiki/core";
import { createJavaScriptRegexEngine } from "shiki/engine/javascript";

/// Grammars we ship, as lazy imports. Single source of truth: the keys
/// give the `Lang` type, the values are registered into the highlighter
/// on first use of that language (see `ensureLang`). The fine-grained
/// `shiki/core` bundle keeps rolldown from emitting a chunk per bundled
/// grammar (~200 of them) that we never load. Add to this *and* to
/// extToLang below. `playmakerfsm` is ours, not from @shikijs/langs: a
/// custom grammar for the backend's PlayMaker FSM pseudocode dump.
const GRAMMARS = {
  xml: () => import("@shikijs/langs/xml"),
  json: () => import("@shikijs/langs/json"),
  lua: () => import("@shikijs/langs/lua"),
  ini: () => import("@shikijs/langs/ini"),
  csharp: () => import("@shikijs/langs/csharp"),
  diff: () => import("@shikijs/langs/diff"),
  glsl: () => import("@shikijs/langs/glsl"),
  cpp: () => import("@shikijs/langs/cpp"),
  playmakerfsm: () => import("./playmakerfsmGrammar"),
} as const;
export type Lang = keyof typeof GRAMMARS;

const THEME = "github-dark";

let highlighterPromise: Promise<HighlighterCore> | null = null;

/// Lazily build a single shared Highlighter (shiki recommends one
/// instance per app). It starts empty — grammar chunks are fetched only
/// for languages actually rendered, so opening a JSON file doesn't pull
/// the lua/glsl/cpp chunks over the network.
export function getHighlighter(): Promise<HighlighterCore> {
  if (!highlighterPromise) {
    highlighterPromise = createHighlighterCore({
      themes: [import("@shikijs/themes/github-dark")],
      langs: [],
      // JS regex engine over oniguruma-wasm: drops the ~600KB wasm chunk.
      // A grammar the JS engine can't express throws here loudly rather
      // than mis-highlighting in silence.
      engine: createJavaScriptRegexEngine(),
    });
  }
  return highlighterPromise;
}

const loadedLangs = new Set<Lang>();

async function ensureLang(lang: Lang): Promise<void> {
  if (loadedLangs.has(lang)) return;
  const h = await getHighlighter();
  await h.loadLanguage(GRAMMARS[lang]());
  loadedLangs.add(lang);
}

const EXT_TO_LANG: Record<string, Lang> = {
  xml: "xml",
  xaml: "xml",
  svg: "xml",
  config: "xml",
  json: "json",
  jsonc: "json",
  lua: "lua",
  ini: "ini",
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
/// Code exercising a grammar's common rules: the JS engine compiles each rule's regex on its
/// first match, so a load alone leaves part of the setup to the first real highlight.
const WARMUP_CODE: Partial<Record<Lang, string>> = {
  csharp: `using System;
public class A : B, IC {
  private static readonly int d = 1; // e
  public string F { get; set; } = "g";
  public override void H(int i) { if (i > 0) { return; } }
}`,
};

/// Loads and exercises `lang` ahead of the content that needs it; for C# that setup took ~45 ms
/// of the first highlight (dev build), now spent while that content is still loading.
export function warmLang(lang: Lang) {
  void highlight(WARMUP_CODE[lang] ?? "", lang);
}

export async function highlight(code: string, lang: Lang): Promise<string> {
  const [h] = await Promise.all([getHighlighter(), ensureLang(lang)]);
  return h.codeToHtml(code, { lang, theme: THEME });
}
