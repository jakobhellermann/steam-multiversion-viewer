// TODO(ai-review): review for style and correctness
export function ErrorBox({ title, error }: { title: string; error: Error | string }) {
  const message = typeof error === "string" ? error : error.message;
  return (
    <div className="mt-4 p-4 border border-red-900 bg-red-950/40 rounded">
      <p className="text-red-400 font-medium mb-1">{title}</p>
      <p className="text-red-300 text-sm font-mono wrap-break-word">{message}</p>
    </div>
  );
}
