// TODO(ai-review): review for style and correctness
import { useEffect, useRef } from "react";

let source: EventSource | null = null;
let subscribers = 0;

/// Listens to one named event of the shared `/api/events` stream, open while anything listens.
export function useServerEvent(name: string, onEvent: (data: unknown) => void) {
  const handler = useRef(onEvent);
  handler.current = onEvent;
  useEffect(() => {
    // EventSource reconnects on its own after errors.
    source ??= new EventSource("/api/events");
    const listener = (e: MessageEvent) => handler.current(JSON.parse(e.data));
    source.addEventListener(name, listener);
    subscribers++;
    return () => {
      source?.removeEventListener(name, listener);
      if (--subscribers === 0) {
        source?.close();
        source = null;
      }
    };
  }, [name]);
}
