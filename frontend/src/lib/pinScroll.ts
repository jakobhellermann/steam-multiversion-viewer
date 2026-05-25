// TODO(ai-review): review for style and correctness

/// Pads the body to its pre-collapse height so a layout shrink doesn't
/// yank the viewport upward. Call before you change DOM that shortens
/// the page; invoke the returned restore() afterwards. The padding
/// shrinks gracefully as the user scrolls back, then clears itself.
export function pinScroll(anchor: HTMLElement | null): () => void {
  const beforeOffset = anchor?.getBoundingClientRect().top;
  const beforeScroll = window.scrollY;
  const beforeHeight = document.documentElement.scrollHeight;
  return () => {
    document.body.style.minHeight = `${beforeHeight}px`;
    window.scrollTo(0, beforeScroll);
    requestAnimationFrame(() => {
      if (anchor && beforeOffset != null) {
        const afterOffset = anchor.getBoundingClientRect().top;
        const delta = afterOffset - beforeOffset;
        if (delta !== 0) window.scrollBy(0, delta);
      }
      const tighten = () => {
        const currentMin = parseInt(document.body.style.minHeight || "0", 10);
        if (!currentMin) {
          window.removeEventListener("scroll", tighten);
          return;
        }
        const needed = window.scrollY + window.innerHeight;
        if (needed < currentMin) {
          document.body.style.minHeight = `${needed}px`;
        } else {
          document.body.style.minHeight = "";
          window.removeEventListener("scroll", tighten);
        }
      };
      window.addEventListener("scroll", tighten, { passive: true });
    });
  };
}
