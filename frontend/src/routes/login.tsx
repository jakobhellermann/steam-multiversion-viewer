// TODO(ai-review): review for style and correctness
import { createFileRoute, redirect, useNavigate } from "@tanstack/react-router";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useState } from "react";
import { fetchAuthStatus, login, submitLoginCode, type AuthStatus } from "../api";
import { ErrorBox } from "../components/ErrorBox";

type Search = {
  /// Where to send the user after a successful login. Defaults to the library.
  redirect?: string;
};

export const Route = createFileRoute("/login")({
  validateSearch: (search: Record<string, unknown>): Search => ({
    redirect: typeof search.redirect === "string" ? search.redirect : undefined,
  }),
  beforeLoad: async ({ context, search }) => {
    // Already authenticated? Don't show the form.
    const status = await context.queryClient.ensureQueryData<AuthStatus>({
      queryKey: ["auth"],
      queryFn: fetchAuthStatus,
    });
    if (status.authenticated) {
      throw redirect({ to: search.redirect ?? "/" });
    }
  },
  component: LoginPage,
});

function LoginPage() {
  const navigate = useNavigate();
  const qc = useQueryClient();
  const { redirect: redirectTo } = Route.useSearch();

  const statusQuery = useQuery({
    queryKey: ["auth"],
    queryFn: fetchAuthStatus,
    // Poll while a login is in flight (any phase except a terminal error).
    refetchInterval: (q) => {
      const d = q.state.data as AuthStatus | undefined;
      const p = d?.pending;
      return p && p.phase !== "error" && !d?.authenticated ? 1500 : false;
    },
  });
  const status = statusQuery.data;
  const pending = status?.pending ?? null;
  const inFlight = !!pending && pending.phase !== "error";
  // The backend keeps the error in `pending` until the next attempt, so this
  // stays put (no polling once errored) instead of flashing for a frame.
  const errorMessage = pending?.phase === "error" ? pending.message : undefined;

  // Once the background login completes, leave the page. Drop anything
  // fetched (and 401'd) while logged out so it refetches — but keep the
  // ["auth"] result we just got, then navigate.
  useEffect(() => {
    if (!status?.authenticated) return;
    qc.removeQueries({ predicate: (q) => q.queryKey[0] !== "auth" });
    void navigate({ to: redirectTo && redirectTo !== "/login" ? redirectTo : "/" });
  }, [status?.authenticated, qc, navigate, redirectTo]);

  return (
    <div className="mx-auto max-w-sm p-8">
      <h1 className="mb-6 text-2xl font-bold">Sign in to Steam</h1>
      {inFlight ? <PendingPanel pending={pending} /> : <CredentialsForm />}
      {errorMessage && <ErrorBox title="Login failed" error={errorMessage} />}
    </div>
  );
}

function CredentialsForm() {
  const qc = useQueryClient();
  const [account, setAccount] = useState("");
  const [password, setPassword] = useState("");

  const mutation = useMutation({
    mutationFn: () => login({ account, password }),
    onSuccess: (s) => qc.setQueryData(["auth"], s),
  });

  const canSubmit = account.trim() !== "" && password !== "" && !mutation.isPending;

  return (
    <form
      className="space-y-4"
      onSubmit={(e) => {
        e.preventDefault();
        if (canSubmit) mutation.mutate();
      }}
    >
      <label className="block">
        <span className="text-sm text-slate-400">Account name</span>
        <input
          type="text"
          value={account}
          onChange={(e) => setAccount(e.target.value)}
          autoFocus
          autoComplete="username"
          className="mt-1 block w-full rounded border border-slate-700 bg-slate-900 px-3 py-2 text-sm"
          spellCheck={false}
        />
      </label>

      <label className="block">
        <span className="text-sm text-slate-400">Password</span>
        <input
          type="password"
          value={password}
          onChange={(e) => setPassword(e.target.value)}
          autoComplete="current-password"
          className="mt-1 block w-full rounded border border-slate-700 bg-slate-900 px-3 py-2 text-sm"
        />
      </label>

      <button
        type="submit"
        disabled={!canSubmit}
        className="w-full rounded bg-sky-700 px-4 py-2 text-sm font-medium hover:bg-sky-600 disabled:opacity-30 disabled:hover:bg-sky-700"
      >
        {mutation.isPending ? "Starting…" : "Sign in"}
      </button>

      {mutation.error && <ErrorBox title="Login failed" error={mutation.error as Error} />}
    </form>
  );
}

function PendingPanel({ pending }: { pending: NonNullable<AuthStatus["pending"]> }) {
  const qc = useQueryClient();
  const [code, setCode] = useState("");

  const codeMutation = useMutation({
    mutationFn: (value: string) => submitLoginCode(value),
    onSuccess: (s) => qc.setQueryData(["auth"], s),
  });

  if (pending.phase === "need_code") {
    const canSubmit = code.trim() !== "" && !codeMutation.isPending;
    return (
      <div className="space-y-4">
        <p className="text-sm text-slate-400">
          Enter the Steam Guard code ({pending.code_type}).
          {pending.details && (
            <span className="block text-xs text-slate-500">{pending.details}</span>
          )}
        </p>
        <form
          className="space-y-4"
          onSubmit={(e) => {
            e.preventDefault();
            if (canSubmit) codeMutation.mutate(code);
          }}
        >
          <input
            type="text"
            value={code}
            onChange={(e) => setCode(e.target.value)}
            autoFocus
            inputMode="numeric"
            autoComplete="one-time-code"
            className="block w-full rounded border border-slate-700 bg-slate-900 px-3 py-2 text-center font-mono text-lg tracking-widest"
            spellCheck={false}
          />
          <button
            type="submit"
            disabled={!canSubmit}
            className="w-full rounded bg-sky-700 px-4 py-2 text-sm font-medium hover:bg-sky-600 disabled:opacity-30 disabled:hover:bg-sky-700"
          >
            {codeMutation.isPending ? "Submitting…" : "Submit code"}
          </button>
        </form>
        {pending.device_available && (
          <p className="text-xs text-slate-500">
            …or just confirm the login in your Steam mobile app — no code needed.
          </p>
        )}
        {codeMutation.error && (
          <ErrorBox title="Could not submit" error={codeMutation.error as Error} />
        )}
      </div>
    );
  }

  // starting / waiting_device
  return (
    <p className="text-sm text-slate-400">Confirm the login in your Steam mobile app to continue…</p>
  );
}
