// TODO(ai-review): review for style and correctness
export function ErrorBox({ title, error }: { title: string; error: Error | string }) {
  const message = typeof error === "string" ? error : error.message;
  return (
    <div className="mt-4 rounded border border-red-900 bg-red-950/40 p-4">
      <p className="mb-1 font-medium text-red-400">{title}</p>
      <p className="font-mono text-sm wrap-break-word text-red-300">{message}</p>
    </div>
  );
}
