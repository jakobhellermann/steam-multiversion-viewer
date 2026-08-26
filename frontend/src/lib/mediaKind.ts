// TODO(ai-review): review for style and correctness

export type MediaKind = "image" | "audio" | "video" | null;

const IMAGE_EXTS = new Set(["png", "jpg", "jpeg", "gif", "webp", "bmp", "svg", "ico", "avif"]);

const AUDIO_EXTS = new Set(["wav", "mp3", "ogg", "flac", "m4a", "opus"]);

const VIDEO_EXTS = new Set(["mp4", "webm", "mov", "m4v"]);

/// Look up which native HTML element (if any) can render the file by
/// extension alone. Browser support varies; this matches what every
/// modern desktop browser handles out of the box.
export function mediaKindForPath(path: string): MediaKind {
  const ext = extOf(path);
  if (!ext) return null;
  if (IMAGE_EXTS.has(ext)) return "image";
  if (AUDIO_EXTS.has(ext)) return "audio";
  if (VIDEO_EXTS.has(ext)) return "video";
  return null;
}

/// Same, but from a MIME type instead of a filename.
export function mediaKindForMime(mime: string): MediaKind {
  if (mime.startsWith("image/")) return "image";
  if (mime.startsWith("audio/")) return "audio";
  if (mime.startsWith("video/")) return "video";
  return null;
}

function extOf(path: string): string | null {
  const slash = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
  const name = slash >= 0 ? path.slice(slash + 1) : path;
  const dot = name.lastIndexOf(".");
  if (dot < 0) return null;
  return name.slice(dot + 1).toLowerCase();
}
