// TODO(ai-review): review for style and correctness
import { createHighlighter, type Highlighter } from "shiki";

/// Languages we ship grammars for. Add to this *and* to extToLang below.
const LANGS = ["xml", "json", "lua", "csharp", "diff"] as const;
type Lang = (typeof LANGS)[number];

const THEME = "github-dark";

let highlighterPromise: Promise<Highlighter> | null = null;

/// Lazily build a single shared Highlighter — shiki recommends one
/// instance per app, and our grammars together weigh a few hundred KB
/// we don't want to ship on first paint.
export function getHighlighter(): Promise<Highlighter> {
  if (!highlighterPromise) {
    highlighterPromise = createHighlighter({
      themes: [THEME],
      langs: LANGS as unknown as string[],
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
  "text/x-csharp": "csharp",
  "application/x-csharp": "csharp",
  "application/json": "json",
  "text/xml": "xml",
  "application/xml": "xml",
  "text/x-lua": "lua",
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
