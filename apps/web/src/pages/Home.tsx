const REPO_URL = "https://github.com/joswayski/captures";
const X_URL = "https://x.com/josevalerio";

export default function Home() {
  return (
    <div className="flex min-h-dvh flex-col">
      <main className="mx-auto flex w-full max-w-3xl flex-1 flex-col justify-center px-6 py-24">
        <p className="inline-flex w-fit items-center gap-2 rounded-full border border-accent/15 bg-accent-soft px-2.5 py-1 text-xs font-medium text-accent">
          <span className="h-1.5 w-1.5 rounded-full bg-accent" />
          Coming soon
        </p>
        <h1 className="mt-4 text-4xl font-semibold tracking-tight text-ink sm:text-5xl">
          Captures
        </h1>
        <p className="mt-4 max-w-lg text-base leading-relaxed text-ink-muted">
          A cross-platform screen capture utility by{" "}
          <a
            href={X_URL}
            target="_blank"
            rel="noreferrer"
            className="font-medium text-ink no-underline underline-offset-2 transition hover:text-accent hover:underline"
          >
            Jose Valerio
          </a>
          .
        </p>
        <div className="mt-8">
          <a
            href={REPO_URL}
            target="_blank"
            rel="noreferrer"
            className="inline-flex items-center gap-2 rounded-md bg-ink px-4 py-2.5 text-sm font-medium text-white no-underline transition hover:bg-zinc-800"
          >
            <GitHubIcon className="h-4 w-4" />
            View on GitHub
          </a>
        </div>
      </main>
    </div>
  );
}

function GitHubIcon({ className }: { className?: string }) {
  return (
    <svg viewBox="0 0 24 24" fill="currentColor" aria-hidden="true" className={className}>
      <path d="M12 2C6.477 2 2 6.486 2 12.021c0 4.425 2.865 8.18 6.839 9.504.5.092.682-.217.682-.483 0-.237-.009-.866-.013-1.7-2.782.605-3.369-1.343-3.369-1.343-.454-1.158-1.11-1.467-1.11-1.467-.908-.621.069-.609.069-.609 1.004.071 1.532 1.032 1.532 1.032.892 1.53 2.341 1.088 2.91.833.091-.647.35-1.088.636-1.339-2.22-.253-4.555-1.113-4.555-4.952 0-1.093.39-1.988 1.029-2.688-.103-.253-.446-1.272.098-2.65 0 0 .84-.27 2.75 1.026A9.564 9.564 0 0 1 12 6.844c.85.004 1.705.115 2.504.337 1.909-1.296 2.747-1.027 2.747-1.027.546 1.379.202 2.398.1 2.651.64.7 1.028 1.595 1.028 2.688 0 3.848-2.339 4.695-4.566 4.944.359.31.678.92.678 1.855 0 1.338-.012 2.419-.012 2.748 0 .268.18.58.688.481A10.02 10.02 0 0 0 22 12.021C22 6.486 17.523 2 12 2Z" />
    </svg>
  );
}
