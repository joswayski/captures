import { useEffect, useRef, useState } from "react";

const REPO_URL = "https://github.com/joswayski/captures";
const X_URL = "https://x.com/josevalerio";
const CONTACT_EMAIL = "contact@josevalerio.com";
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
      <main className="mx-auto flex w-full max-w-2xl flex-1 flex-col px-6 py-16 sm:py-24">
        <section>
          <div className="flex items-center gap-3">
            <span className="brand-mark" aria-hidden="true">
              <CaptureIcon className="h-4 w-4" />
            </span>
            <h1 className="text-2xl font-medium tracking-tight text-ink sm:text-3xl">Captures</h1>
          </div>

          <p className="mt-5 max-w-xl text-sm leading-relaxed text-ink-muted sm:text-[0.9375rem]">
            A cross-platform screen capture utility by{" "}
            <a
              href={X_URL}
              target="_blank"
              rel="noreferrer"
              className="font-medium text-ink no-underline underline-offset-2 transition-colors duration-200 ease-out hover:underline"
            >
              Jose Valerio
            </a>
            .
          </p>

          <div className="mt-12 border-t border-border pt-10">
            <p className="text-[0.6875rem] font-medium uppercase tracking-[0.14em] text-accent-readable">
              Experimental preview
            </p>
            <h2 className="mt-2 text-base font-medium tracking-tight text-ink sm:text-lg">
              Download Captures
            </h2>
            <p className="mt-3 max-w-xl text-sm leading-relaxed text-ink-muted">
              Builds are available after every merge and may contain bugs or incomplete features.
              Please give feedback on{" "}
              <a
                href={X_URL}
                target="_blank"
                rel="noreferrer"
                className="inline-chip"
                aria-label="Give feedback on X"
              >
                <XIcon className="h-3 w-3" />
              </a>
              ,{" "}
              <a
                href={REPO_URL}
                target="_blank"
                rel="noreferrer"
                className="inline-chip"
                aria-label="Give feedback on GitHub"
              >
                <GitHubIcon className="h-3 w-3" />
                GitHub
              </a>
              , or by{" "}
              <CopyEmailButton email={CONTACT_EMAIL} />.
            </p>

            <ul className="mt-6 divide-y divide-border border border-border bg-surface">
              {PREVIEW_DOWNLOADS.map((download) => (
                <li key={download.href}>
                  <a
                    href={download.href}
                    className="group flex items-center justify-between gap-4 px-4 py-3.5 no-underline transition-colors duration-200 ease-out hover:bg-canvas"
                  >
                    <span className="min-w-0">
                      <span className="block text-sm font-medium text-ink">{download.platform}</span>
                      <span className="mt-0.5 block text-xs text-ink-soft">{download.detail}</span>
                    </span>
                    <span className="shrink-0 text-xs font-medium text-ink-muted transition-colors duration-200 ease-out group-hover:text-accent-readable">
                      Download
                      <span className="sr-only"> {download.fileName}</span>
                    </span>
                  </a>
                </li>
              ))}
            </ul>
          </div>
        </section>

        <section aria-labelledby="latest-changes-heading" className="mt-14 border-t border-border pt-10">
          <h2
            id="latest-changes-heading"
            className="text-[0.6875rem] font-medium uppercase tracking-[0.14em] text-accent-readable"
          >
            Latest changes
          </h2>

          <ol className="mt-6 space-y-5">
            {__LATEST_CHANGES__.map((change) => (
              <li key={change.sha}>
                <a
                  href={change.url}
                  target="_blank"
                  rel="noreferrer"
                  className="text-sm font-medium leading-snug text-ink no-underline underline-offset-4 transition-colors duration-200 ease-out hover:underline"
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

function CopyEmailButton({ email }: { email: string }) {
  const [copied, setCopied] = useState(false);
  const resetTimer = useRef<number | null>(null);

  useEffect(() => {
    return () => {
      if (resetTimer.current !== null) window.clearTimeout(resetTimer.current);
    };
  }, []);

  async function handleCopy() {
    try {
      await navigator.clipboard.writeText(email);
    } catch {
      // Fallback for older browsers or denied clipboard permission.
      const textarea = document.createElement("textarea");
      textarea.value = email;
      textarea.setAttribute("readonly", "");
      textarea.style.position = "fixed";
      textarea.style.opacity = "0";
      document.body.appendChild(textarea);
      textarea.select();
      document.execCommand("copy");
      document.body.removeChild(textarea);
    }

    setCopied(true);
    if (resetTimer.current !== null) window.clearTimeout(resetTimer.current);
    resetTimer.current = window.setTimeout(() => setCopied(false), 1_800);
  }

  return (
    <button
      type="button"
      className="group/email inline-chip relative"
      onClick={handleCopy}
      aria-label={copied ? `Copied ${email}` : `Copy email ${email}`}
    >
      <MailIcon className="h-3 w-3" />
      email
      <span
        role="status"
        className={[
          "pointer-events-none absolute bottom-full left-1/2 z-10 mb-1.5 -translate-x-1/2 whitespace-nowrap rounded bg-ink px-2 py-1 text-[0.6875rem] font-medium text-accent-ink shadow-sm transition-opacity duration-150 ease-out",
          copied ? "opacity-100" : "opacity-0 group-hover/email:opacity-100 group-focus-visible/email:opacity-100",
        ].join(" ")}
      >
        {copied ? "Copied!" : "Click to copy"}
      </span>
    </button>
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

function XIcon({ className }: { className?: string }) {
  return (
    <svg viewBox="0 0 24 24" fill="currentColor" aria-hidden="true" className={className}>
      <path d="M18.244 2.25h3.308l-7.227 8.26 8.502 11.24H16.17l-4.714-6.231-5.401 6.231H2.744l7.727-8.835L1.254 2.25H8.08l4.253 5.622zm-1.161 17.52h1.833L7.084 4.126H5.117z" />
    </svg>
  );
}

function MailIcon({ className }: { className?: string }) {
  return (
    <svg
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.8"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      className={className}
    >
      <rect x="3" y="5" width="18" height="14" rx="2" />
      <path d="m4 7 8 6 8-6" />
    </svg>
  );
}
