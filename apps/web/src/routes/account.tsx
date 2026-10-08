import { createFileRoute } from "@tanstack/react-router";

export const Route = createFileRoute("/account")({
  head: () => ({ meta: [{ title: "Account — Captures" }, { name: "robots", content: "noindex" }] }),
  component: AccountPage,
});

function AccountPage() {
  return (
    <main className="mx-auto flex min-h-dvh w-full max-w-xl flex-col justify-center px-6 py-12">
      <a href="/" className="mb-10 text-sm text-ink-muted underline-offset-4 hover:underline">
        ← Captures
      </a>
      <p className="text-sm text-ink-soft">Optional account</p>
      <h1 className="mt-2 text-3xl font-medium tracking-tight text-ink">Your Captures account</h1>
      <p className="mt-4 text-sm leading-relaxed text-ink-muted">
        Screenshots, GIFs, and recordings work without an account. Accounts are planned for future
        cloud features; uploads and sharing are not available yet.
      </p>
      <section
        aria-label="Account status"
        className="mt-8 rounded-xl border border-border bg-surface p-6"
      >
        <h2 className="font-medium text-ink">Accounts aren’t available right now</h2>
        <p role="status" className="mt-3 text-sm leading-relaxed text-ink-muted">
          Sign-in and account creation are not available. You can still use Captures without an
          account.
        </p>
        <a href="/" className="inline-chip mt-6 px-3 py-2">
          Back to Captures
        </a>
      </section>
    </main>
  );
}
