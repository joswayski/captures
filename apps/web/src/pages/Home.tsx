import { useEffect, useRef, useState } from "react";
import { isWithinCookingWindow } from "../cookingPreview";
import type { PreviewDownloadId } from "../detectPreviewDownload";
import ProductGallery from "../ProductGallery";

const REPO_URL = "https://github.com/joswayski/captures";
const RELEASES_URL = `${REPO_URL}/releases`;
const X_URL = "https://x.com/josevalerio";
const CONTACT_EMAIL = "contact@josevalerio.com";
const PREVIEW_DOWNLOAD_BASE = `${REPO_URL}/releases/download/preview`;
const COOKING_TOOLTIP =
  "Preview builds are still publishing this change. Downloads may not include it yet.";

const PREVIEW_DOWNLOADS = [
  {
    id: "macos",
    family: "macos",
    platform: "macOS 13+",
    arch: "Apple silicon",
    format: "dmg",
    label: "Download for macOS",
    href: `${PREVIEW_DOWNLOAD_BASE}/Captures-macOS-Apple-Silicon.dmg`,
    fileName: "Captures-macOS-Apple-Silicon.dmg",
  },
  {
    id: "windows",
    family: "windows",
    platform: "Windows 11",
    arch: "x64",
    format: "exe",
    label: "Download for Windows",
    href: `${PREVIEW_DOWNLOAD_BASE}/Captures-Windows-x64-setup.exe`,
    fileName: "Captures-Windows-x64-setup.exe",
  },
  {
    id: "linux-deb",
    family: "linux",
    platform: "Ubuntu / Debian",
    arch: "x64",
    format: "deb",
    label: "Download for Linux",
    href: `${PREVIEW_DOWNLOAD_BASE}/Captures-Linux-x64.deb`,
    fileName: "Captures-Linux-x64.deb",
  },
  {
    id: "linux-appimage",
    family: "linux",
    platform: "Other Linux",
    arch: "x64",
    format: "AppImage",
    label: "Download for Linux",
    href: `${PREVIEW_DOWNLOAD_BASE}/Captures-Linux-x64.AppImage`,
    fileName: "Captures-Linux-x64.AppImage",
  },
] as const;

type PreviewDownload = (typeof PREVIEW_DOWNLOADS)[number];
type OsFamily = PreviewDownload["family"];

const relativeTimeFormatter = new Intl.RelativeTimeFormat("en", {
  numeric: "always",
});

type HomeProps = {
  initialNow: number;
  latestChanges: readonly LatestChange[];
  previewDownloadId: PreviewDownloadId | null;
  cookingShas: readonly string[];
};

export default function Home({
  initialNow,
  latestChanges,
  previewDownloadId,
  cookingShas,
}: HomeProps) {
  const [now, setNow] = useState(initialNow);
  const detectedDownload = previewDownloadById(previewDownloadId);
  const linuxAlternative = detectedDownload ? linuxAlternativeDownload(detectedDownload) : null;

  useEffect(() => {
    setNow(Date.now());
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

          <div className="mt-12 border-t border-border pt-10" id="download">
            {detectedDownload ? (
              <>
                <h2 className="text-base font-medium tracking-tight text-ink sm:text-lg">
                  Download Captures{" "}
                  <span className="text-xs font-normal text-ink-soft">(experimental)</span>
                </h2>
                <p className="mt-3 max-w-xl text-sm leading-relaxed text-ink-muted">
                  Builds are available after every merge and may contain bugs or incomplete
                  features. Please give feedback in the app, on{" "}
                  <a
                    href={X_URL}
                    target="_blank"
                    rel="noreferrer"
                    className="inline-chip"
                    aria-label="Give feedback on X"
                  >
                    <XIcon className="h-3 w-3" />
                  </a>
                  , or{" "}
                  <CopyEmailButton email={CONTACT_EMAIL} />.
                </p>

                <div className="mt-8">
                  <div className="flex flex-col gap-2.5 sm:flex-row sm:flex-wrap sm:items-center">
                    <a href={detectedDownload.href} className="download-button">
                      <OsIcon family={detectedDownload.family} className="h-[1.15rem] w-[1.15rem]" />
                      {detectedDownload.label}
                      <span className="sr-only"> {detectedDownload.fileName}</span>
                    </a>
                    <GitHubSourceButton />
                  </div>
                  {linuxAlternative ? (
                    <p className="mt-3 text-xs text-ink-soft">
                      <a
                        href={linuxAlternative.href}
                        className="font-medium text-ink-muted no-underline underline-offset-2 transition-colors duration-200 ease-out hover:text-accent-readable hover:underline"
                      >
                        or {linuxAlternativeLabel(linuxAlternative)}
                      </a>
                    </p>
                  ) : null}
                  <p className="mt-3 text-xs text-ink-soft">
                    <a
                      href={RELEASES_URL}
                      target="_blank"
                      rel="noreferrer"
                      className="text-ink-muted no-underline underline-offset-2 transition-colors duration-200 ease-out hover:text-accent-readable hover:underline"
                    >
                      {availabilityLabel(detectedDownload)}
                    </a>
                  </p>
                </div>
              </>
            ) : (
              <>
                <h2 className="text-base font-medium tracking-tight text-ink sm:text-lg">
                  Capture from your desktop
                </h2>
                <p className="mt-3 max-w-xl text-sm leading-relaxed text-ink-muted">
                  Captures is available for macOS, Windows, and Linux. Visit from your computer to
                  download it, or follow the project on GitHub.
                </p>
                <div className="mt-8">
                  <GitHubSourceButton primary />
                </div>
              </>
            )}
          </div>
        </section>

        <ProductGallery />

        <section aria-labelledby="latest-changes-heading" className="mt-14 border-t border-border pt-10">
          <h2
            id="latest-changes-heading"
            className="text-[0.6875rem] font-medium uppercase tracking-[0.14em] text-accent-readable"
          >
            Latest changes
          </h2>

          <ol className="mt-6 space-y-5">
            {latestChanges.map((change) => {
              const cooking =
                cookingShas.includes(change.sha) &&
                isWithinCookingWindow(change.committedAt, now);
              const cookingTipId = `cooking-tip-${change.sha.slice(0, 12)}`;
              return (
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
                    {cooking ? (
                      <>
                        <span aria-hidden="true"> · </span>
                        <span
                          className="cooking-status"
                          tabIndex={0}
                          aria-describedby={cookingTipId}
                        >
                          <span className="cooking-emoji" aria-hidden="true">
                            🍳
                          </span>{" "}
                          still cooking
                          <span id={cookingTipId} role="tooltip" className="cooking-tooltip">
                            {COOKING_TOOLTIP}
                          </span>
                        </span>
                      </>
                    ) : null}
                  </p>
                </li>
              );
            })}
          </ol>
        </section>
      </main>
    </div>
  );
}

function previewDownloadById(id: PreviewDownloadId | null) {
  return PREVIEW_DOWNLOADS.find((download) => download.id === id) ?? null;
}

function linuxAlternativeDownload(download: PreviewDownload) {
  if (download.family !== "linux") return null;
  return PREVIEW_DOWNLOADS.find((item) => item.family === "linux" && item.id !== download.id) ?? null;
}

function linuxAlternativeLabel(download: PreviewDownload) {
  return download.id === "linux-deb" ? ".deb for Ubuntu / Debian" : "AppImage";
}

function availabilityLabel(download: PreviewDownload | null) {
  const names = ["macOS", "Windows", "Linux"] as const;
  const current =
    download?.family === "macos"
      ? "macOS"
      : download?.family === "windows"
        ? "Windows"
        : download?.family === "linux"
          ? "Linux"
          : null;
  if (!current) return "Available for macOS, Windows, and Linux";
  const others = names.filter((name) => name !== current);
  return `Also available for ${others[0]} and ${others[1]}`;
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
      className="group/email inline-flex max-w-full translate-y-px cursor-pointer flex-col items-start rounded border border-border bg-surface px-2 py-1 text-left no-underline transition-colors duration-200 ease-out hover:border-accent/30 hover:bg-accent-soft focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent"
      onClick={handleCopy}
      aria-label={copied ? `Copied ${email}` : `Copy email ${email}`}
    >
      <span className="inline-flex min-w-0 items-center gap-1.5 text-[0.8125rem] font-medium leading-none text-ink transition-colors duration-200 ease-out group-hover/email:text-accent-readable">
        <MailIcon className="h-3 w-3 shrink-0" />
        <span className="break-all">{email}</span>
      </span>
      <span
        role="status"
        className="mt-1 text-[0.625rem] font-normal leading-none text-ink-soft transition-colors duration-200 ease-out group-hover/email:text-accent-readable/70"
      >
        {copied ? "copied!" : "click to copy"}
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
      width="16"
      height="16"
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

function GitHubSourceButton({ primary = false }: { primary?: boolean }) {
  return (
    <a
      href={REPO_URL}
      target="_blank"
      rel="noreferrer"
      className={primary ? "download-button" : "source-button"}
    >
      <GitHubIcon className="h-[1.15rem] w-[1.15rem]" />
      {primary ? "View on GitHub" : "View source"}
    </a>
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

function OsIcon({ family, className }: { family: OsFamily; className?: string }) {
  if (family === "macos") return <AppleIcon className={className} />;
  if (family === "windows") return <WindowsIcon className={className} />;
  return <LinuxIcon className={className} />;
}

function AppleIcon({ className }: { className?: string }) {
  return (
    <svg viewBox="0 0 24 24" fill="currentColor" aria-hidden="true" className={className}>
      <path d="M12.152 6.896c-.948 0-2.415-1.078-3.96-1.04-2.04.027-3.91 1.183-4.961 3.014-2.117 3.675-.546 9.103 1.519 12.09 1.013 1.454 2.208 3.09 3.792 3.039 1.52-.065 2.09-.987 3.935-.987 1.831 0 2.35.987 3.96.948 1.637-.026 2.676-1.48 3.676-2.948 1.156-1.688 1.636-3.325 1.662-3.415-.039-.013-3.182-1.221-3.22-4.857-.026-3.04 2.48-4.494 2.597-4.559-1.429-2.09-3.623-2.324-4.39-2.376-2-.156-3.675 1.09-4.61 1.09zM15.53 3.83c.843-1.012 1.4-2.427 1.245-3.83-1.207.052-2.662.805-3.532 1.818-.78.896-1.454 2.338-1.273 3.714 1.338.104 2.715-.688 3.559-1.701" />
    </svg>
  );
}

function WindowsIcon({ className }: { className?: string }) {
  return (
    <svg viewBox="0 0 24 24" fill="currentColor" aria-hidden="true" className={className}>
      <path d="M3 5.15 11.15 4v7.35H3V5.15Zm9.15-1.45L21 2.4v8.95h-8.85V3.7ZM3 12.85h8.15V20.2L3 19.05v-6.2Zm9.15 0H21v8.95l-8.85-1.45v-7.5Z" />
    </svg>
  );
}

function LinuxIcon({ className }: { className?: string }) {
  return (
    <svg viewBox="0 0 24 24" fill="currentColor" aria-hidden="true" className={className}>
      <path
        fillRule="evenodd"
        d="M12 2.2c-2.05 0-3.7 1.7-3.7 4 0 .72.18 1.38.5 1.94-2.28 1.32-3.85 3.55-4.38 6.1-.52 2.5.05 4.28 1.55 5.15-.62.42-1.02 1.1-1.02 1.88 0 1.42 1.48 2.18 3.05 2.52 1.42.3 3.12.36 5 .36s3.58-.06 5-.36c1.57-.34 3.05-1.1 3.05-2.52 0-.78-.4-1.46-1.02-1.88 1.5-.87 2.07-2.65 1.55-5.15-.53-2.55-2.1-4.78-4.38-6.1.32-.56.5-1.22.5-1.94 0-2.3-1.65-4-3.7-4ZM10.2 5.55c.45-.3.95.05.88.58-.06.46-.58.72-1.02.45-.44-.26-.42-.82.14-1.03Zm3.8 0c.56.21.58.77.14 1.03-.44.27-.96.01-1.02-.45-.07-.53.43-.88.88-.58ZM9.35 16.85c.9.5 1.75.78 2.65.78s1.75-.28 2.65-.78c.28-.16.6.02.6.34 0 .78-1.05 1.5-3.25 1.5s-3.25-.72-3.25-1.5c0-.32.32-.5.6-.34Z"
      />
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
