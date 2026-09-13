// TODO(ai-review): review for style and correctness
import { useEffect, useRef } from "react";

/// Inline audio/video player. Focuses itself so space toggles play
/// instead of scrolling the page.
export function MediaPlayer({ kind, src }: { kind: "audio" | "video"; src: string }) {
  const ref = useRef<HTMLMediaElement>(null);
  useEffect(() => {
    ref.current?.focus();
  }, [src]);

  if (kind === "audio") {
    return (
      <audio
        ref={ref as React.RefObject<HTMLAudioElement>}
        src={src}
        controls
        className={"w-full"}
      />
    );
  }
  return (
    <video
      ref={ref as React.RefObject<HTMLVideoElement>}
      src={src}
      controls
      className="max-w-full rounded surface-well"
    />
  );
}
