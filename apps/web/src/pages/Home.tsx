import { useEffect, useState } from "react";

const REPO_URL = "https://github.com/joswayski/captures";
const X_URL = "https://x.com/josevalerio";
const EMAIL_URL = "mailto:contact@josevalerio.com";
const PREVIEW_DOWNLOAD_BASE = `${REPO_URL}/releases/download/preview`;

const PREVIEW_DOWNLOADS = [
  {
    platform: "macOS 13+",
    detail: "Apple silicon · .dmg",
    href: `${PREVIEW_DOWNLOAD_BASE}/Captures-macOS-Apple-Silicon.dmg`,
    fileName: "Captures-macOS-Apple-Silicon.dmg",
  },
  {
    platform: "Windows 11",
    detail: "x64 · .exe",
    href: `${PREVIEW_DOWNLOAD_BASE}/Captures-Windows-x64-setup.exe`,
    fileName: "Captures-Windows-x64-setup.exe",
  },
  {
    platform: "Ubuntu / Debian",
    detail: "x64 · .deb",
    href: `${PREVIEW_DOWNLOAD_BASE}/Captures-Linux-x64.deb`,
    fileName: "Captures-Linux-x64.deb",
  },
  {
    platform: "Other Linux",
    detail: "x64 · AppImage",
    href: `${PREVIEW_DOWNLOAD_BASE}/Captures-Linux-x64.AppImage`,
    fileName: "Captures-Linux-x64.AppImage",
  },
] as const;

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
          <p className="inline-flex w-fit items-center gap-2 rounded-full border border-accent/20 bg-accent-soft px-2.5 py-1 text-xs font-medium text-accent-readable">
            <span className="h-1.5 w-1.5 rounded-full bg-accent" />
            Work in progress
          </p>
          <div className="mt-4 flex items-center gap-3">
            <span className="brand-symbol" aria-hidden="true">
              <CaptureIcon className="h-6 w-6" />
            </span>
            <h1 className="text-4xl font-semibold tracking-tight text-ink sm:text-5xl">
              Captures
            </h1>
          </div>
          <p className="mt-4 max-w-lg text-base leading-relaxed text-ink-muted">
            A cross-platform screen capture utility by{" "}
            <a
              href={X_URL}
              target="_blank"
              rel="noreferrer"
              className="font-medium text-ink no-underline underline-offset-2 transition-colors duration-300 ease-out hover:text-accent-readable hover:underline"
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
              className="inline-flex cursor-pointer items-center gap-2 rounded-md bg-accent px-4 py-2.5 text-sm font-medium text-accent-ink no-underline transition-colors duration-300 ease-out hover:bg-accent-hover"
            >
              <GitHubIcon className="h-4 w-4" />
              View on GitHub
            </a>
          </div>

          <div className="mt-12 max-w-xl">
            <h2 className="text-lg font-semibold tracking-tight text-ink">
              Download Captures Experimental Preview
            </h2>
            <p className="mt-2 text-sm leading-relaxed text-ink-muted">
              Builds are available after every merge and may contain bugs or incomplete features.
              Please give feedback on{" "}
              <a
                href={X_URL}
                target="_blank"
                rel="noreferrer"
                className="font-medium text-ink no-underline underline-offset-2 transition-colors duration-300 ease-out hover:text-accent-readable hover:underline"
              >
                X
              </a>
              ,{" "}
              <a
                href={REPO_URL}
                target="_blank"
                rel="noreferrer"
                className="font-medium text-ink no-underline underline-offset-2 transition-colors duration-300 ease-out hover:text-accent-readable hover:underline"
              >
                GitHub
              </a>
              , or by{" "}
              <a
                href={EMAIL_URL}
                className="font-medium text-ink no-underline underline-offset-2 transition-colors duration-300 ease-out hover:text-accent-readable hover:underline"
              >
                email
              </a>
              !
            </p>

            <ul className="mt-5 grid gap-3 sm:grid-cols-2">
              {PREVIEW_DOWNLOADS.map((download) => (
                <li key={download.href}>
                  <a
                    href={download.href}
                    className="group flex h-full cursor-pointer flex-col rounded-lg border border-border bg-surface px-4 py-3.5 no-underline shadow-[0_1px_0_rgba(29,29,31,0.03)] transition-colors duration-300 ease-out hover:border-border-strong hover:bg-canvas"
                  >
                    <span className="text-sm font-medium text-ink transition-colors duration-300 ease-out group-hover:text-accent-readable">
                      {download.platform}
                    </span>
                    <span className="mt-1 text-xs text-ink-soft">{download.detail}</span>
                    <span className="mt-3 text-xs font-medium text-accent-readable">
                      Download
                      <span className="sr-only"> {download.fileName}</span>
                    </span>
                  </a>
                </li>
              ))}
            </ul>
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
                  className="font-medium leading-snug text-ink no-underline underline-offset-4 transition-colors duration-300 ease-out hover:text-accent-readable hover:underline"
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

function CaptureIcon({ className }: { className?: string }) {
  return (
    <svg
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      aria-hidden="true"
      className={className}
    >
      <path d="M9 4H6a2 2 0 0 0-2 2v3M15 4h3a2 2 0 0 1 2 2v3M20 15v3a2 2 0 0 1-2 2h-3M9 20H6a2 2 0 0 1-2-2v-3" />
      <path
        className="capture-icon-spark"
        d="M12 8.8c.45 1.65 1.55 2.75 3.2 3.2-1.65.45-2.75 1.55-3.2 3.2-.45-1.65-1.55-2.75-3.2-3.2 1.65-.45 2.75-1.55 3.2-3.2Z"
      />
    </svg>
  );
}

function GitHubIcon({ className }: { className?: string }) {
  return (
    <svg viewBox="0 0 24 24" fill="currentColor" aria-hidden="true" className={className}>
      <path d="M12 2C6.477 2 2 6.486 2 12.021c0 4.425 2.865 8.18 6.839 9.504.5.092.682-.217.682-.483 0-.237-.009-.866-.013-1.7-2.782.605-3.369-1.343-3.369-1.343-.454-1.158-1.11-1.467-1.11-1.467-.908-.621.069-.609.069-.609 1.004.071 1.532 1.032 1.532 1.032.892 1.53 2.341 1.088 2.91.833.091-.647.35-1.088.636-1.339-2.22-.253-4.555-1.113-4.555-4.952 0-1.093.39-1.988 1.029-2.688-.103-.253-.446-1.272.098-2.65 0 0 .84-.27 2.75 1.026A9.564 9.564 0 0 1 12 6.844c.85.004 1.705.115 2.504.337 1.909-1.296 2.747-1.027 2.747-1.027.546 1.379.202 2.398.1 2.651.64.7 1.028 1.595 1.028 2.688 0 3.848-2.339 4.695-4.566 4.944.359.31.678.92.678 1.855 0 1.338-.012 2.419-.012 2.748 0 .268.18.58.688.481A10.02 10.02 0 0 0 22 12.021C22 6.486 17.523 2 12 2Z" />
    </svg>
  );
}
