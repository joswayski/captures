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
      <h1 className="mt-2 text-3xl font-medium tracking-tight text-ink">Captures accounts</h1>
      <p className="mt-4 text-sm leading-relaxed text-ink-muted">
        Screenshots, GIFs, and recordings work completely offline with no account required.
      </p>
      <section
        aria-label="Account status"
        className="mt-8 rounded-xl border border-border bg-surface p-6"
      >
        <h2 className="font-medium text-ink">Not available yet</h2>
        <p role="status" className="mt-3 text-sm leading-relaxed text-ink-muted">
          Accounts are planned for future cloud uploads and link sharing, and aren’t open for
          sign-in yet.
        </p>
      </section>
    </main>
  );
}
