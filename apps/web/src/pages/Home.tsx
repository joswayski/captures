import { useEffect, useState } from "react";

const REPO_URL = "https://github.com/joswayski/captures";
const X_URL = "https://x.com/josevalerio";

const relativeTimeFormatter = new Intl.RelativeTimeFormat("en", {
  numeric: "always",
});

export default function Home() {
  const [now, setNow] = useState(() => Date.now());

  useEffect(() => {
    const interval = window.setInterval(() => setNow(Date.now()), 60_000);
    return () => window.clearInterval(interval);
  }, []);

  return (
    <div className="flex min-h-dvh flex-col">
      <main className="mx-auto flex w-full max-w-3xl flex-1 flex-col px-6 py-20 sm:py-24">
        <section className="py-8 sm:py-12">
          <p className="inline-flex w-fit items-center gap-2 rounded-full border border-accent/15 bg-accent-soft px-2.5 py-1 text-xs font-medium text-accent">
            <span className="h-1.5 w-1.5 rounded-full bg-accent" />
            Work in progress
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
              className="font-medium text-ink no-underline underline-offset-2 transition-colors duration-300 ease-out hover:text-accent hover:underline"
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
              className="inline-flex items-center gap-2 rounded-md bg-ink px-4 py-2.5 text-sm font-medium text-white no-underline transition-colors duration-300 ease-out hover:bg-accent"
            >
              <GitHubIcon className="h-4 w-4" />
              View on GitHub
            </a>
          </div>
        </section>

        <section aria-labelledby="latest-changes-heading" className="mt-8 border-t border-border pt-10">
          <h2 id="latest-changes-heading" className="text-2xl font-semibold tracking-tight text-ink">
            Latest changes
          </h2>

          <ol className="ml-1.5 mt-7 border-l border-border">
            {__LATEST_CHANGES__.map((change) => (
              <li key={change.sha} className="relative pb-7 pl-7 last:pb-0">
                <span className="absolute -left-[5px] top-1.5 h-2.5 w-2.5 rounded-full border-2 border-canvas bg-accent ring-1 ring-accent/25" />
                <a
                  href={change.url}
                  target="_blank"
                  rel="noreferrer"
                  className="font-medium leading-snug text-ink no-underline underline-offset-4 transition-colors duration-300 ease-out hover:text-accent hover:underline"
                >
                  {change.title}
                </a>
                <p className="mt-1.5 text-xs text-ink-soft">
                  <time dateTime={change.committedAt}>{formatRelativeTime(change.committedAt, now)}</time>
                  <span aria-hidden="true"> · </span>
                  {change.pullRequest ? `PR #${change.pullRequest}` : change.sha}
                </p>
              </li>
            ))}
          </ol>
        </section>
      </main>
    </div>
  );
}

function formatRelativeTime(date: string, now: number) {
  const secondsFromNow = (new Date(date).getTime() - now) / 1_000;
  const divisions = [
    { amount: 60, unit: "second" },
    { amount: 60, unit: "minute" },
    { amount: 24, unit: "hour" },
    { amount: 7, unit: "day" },
    { amount: 4.345, unit: "week" },
    { amount: 12, unit: "month" },
    { amount: Number.POSITIVE_INFINITY, unit: "year" },
  ] as const;

  let duration = secondsFromNow;
  for (const division of divisions) {
    if (Math.abs(duration) < division.amount) {
      return relativeTimeFormatter.format(Math.round(duration), division.unit);
    }
    duration /= division.amount;
  }

  return relativeTimeFormatter.format(0, "second");
}

function GitHubIcon({ className }: { className?: string }) {
  return (
    <svg viewBox="0 0 24 24" fill="currentColor" aria-hidden="true" className={className}>
      <path d="M12 2C6.477 2 2 6.486 2 12.021c0 4.425 2.865 8.18 6.839 9.504.5.092.682-.217.682-.483 0-.237-.009-.866-.013-1.7-2.782.605-3.369-1.343-3.369-1.343-.454-1.158-1.11-1.467-1.11-1.467-.908-.621.069-.609.069-.609 1.004.071 1.532 1.032 1.532 1.032.892 1.53 2.341 1.088 2.91.833.091-.647.35-1.088.636-1.339-2.22-.253-4.555-1.113-4.555-4.952 0-1.093.39-1.988 1.029-2.688-.103-.253-.446-1.272.098-2.65 0 0 .84-.27 2.75 1.026A9.564 9.564 0 0 1 12 6.844c.85.004 1.705.115 2.504.337 1.909-1.296 2.747-1.027 2.747-1.027.546 1.379.202 2.398.1 2.651.64.7 1.028 1.595 1.028 2.688 0 3.848-2.339 4.695-4.566 4.944.359.31.678.92.678 1.855 0 1.338-.012 2.419-.012 2.748 0 .268.18.58.688.481A10.02 10.02 0 0 0 22 12.021C22 6.486 17.523 2 12 2Z" />
    </svg>
  );
}
