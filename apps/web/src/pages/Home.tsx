import { useEffect, useRef, useState } from "react";
import { isWithinCookingWindow } from "../cookingPreview";
import type { PreviewDownloadId } from "../detectPreviewDownload";
import ProductGallery from "../ProductGallery";

const REPO_URL = "https://github.com/joswayski/captures";
const X_URL = "https://x.com/josevalerio";
const CAPER_INVITE_URL = "https://caper.chat/invite/pIXKAZFUp8";
const AUTHOR_URL = "https://josevalerio.com";
const CONTACT_EMAIL = "jose@captur.es";
const PREVIEW_DOWNLOAD_BASE = `${REPO_URL}/releases/download/preview`;
const COOKING_TOOLTIP = "Still building. This change may not be in the download yet.";

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

  useEffect(() => {
    setNow(Date.now());
    const interval = window.setInterval(() => setNow(Date.now()), 60_000);
    return () => window.clearInterval(interval);
  }, []);

  return (
    <div className="flex min-h-dvh flex-col overflow-x-clip">
      <main className="mx-auto flex w-full max-w-2xl flex-1 flex-col px-6 py-16 sm:py-24">
        <section>
          <div className="flex items-center gap-3">
            <span className="brand-mark" aria-hidden="true">
              <CaptureIcon className="h-5 w-5" />
            </span>
            <h1 className="text-2xl font-medium tracking-tight text-ink sm:text-3xl">Captures</h1>
          </div>

          <p className="mt-5 max-w-xl text-sm leading-relaxed text-ink-muted sm:text-[0.9375rem]">
            A cross-platform screen capture utility by{" "}
            <a
              href={AUTHOR_URL}
              target="_blank"
              rel="noreferrer"
              className="font-medium text-ink no-underline underline-offset-2 transition-colors duration-200 ease-out hover:underline"
            >
              Jose Valerio
            </a>
            .
          </p>

          <div className="mt-8" id="download">
            {detectedDownload ? (
              <>
                <div className="flex flex-col gap-2.5 sm:flex-row sm:flex-wrap sm:items-center">
                  {detectedDownload.family === "linux" ? (
                    <LinuxDownloadPicker />
                  ) : (
                    <a href={detectedDownload.href} className="download-button">
                      <OsIcon
                        family={detectedDownload.family}
                        className="h-[1.15rem] w-[1.15rem]"
                      />
                      {detectedDownload.label}
                      <span className="sr-only"> {detectedDownload.fileName}</span>
                    </a>
                  )}
                  <GitHubSourceButton />
                </div>
                <FeedbackList appMenu={detectedDownload.family === "macos" ? "menu bar" : "tray"} />
              </>
            ) : (
              <>
                <p className="max-w-xl text-sm leading-relaxed text-ink-muted">
                  Captures runs on macOS, Windows, and Linux. Open this page on your computer to
                  download it.
                </p>
                <div className="mt-8">
                  <GitHubSourceButton primary />
                </div>
                <FeedbackList />
              </>
            )}
          </div>
        </section>

        <ProductGallery />

        <section
          aria-labelledby="latest-changes-heading"
          className="mt-14 border-t border-border pt-10"
        >
          <h2
            id="latest-changes-heading"
            className="text-[0.6875rem] font-medium uppercase tracking-[0.14em] text-accent-readable"
          >
            Development updates
          </h2>

          <ol className="mt-6 space-y-5">
            {latestChanges.map((change) => {
              const cooking =
                cookingShas.includes(change.sha) && isWithinCookingWindow(change.committedAt, now);
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
                    <time dateTime={change.committedAt}>
                      {formatRelativeTime(change.committedAt, now)}
                    </time>
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

// Browsers report Linux but rarely the distribution, so Linux visitors pick
// the package themselves, as on caper.chat.
const LINUX_DOWNLOADS = [
  { id: "linux-deb", name: "Ubuntu / Debian", detail: ".deb · x64" },
  {
    id: "linux-appimage",
    name: "Other Linux",
    detail: "AppImage · x64 · runs on most distributions",
  },
] as const;

function LinuxDownloadPicker() {
  const [open, setOpen] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);
  const buttonRef = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    if (!open) return;
    function onPointerDown(event: PointerEvent) {
      if (!rootRef.current?.contains(event.target as Node)) setOpen(false);
    }
    function onKeyDown(event: KeyboardEvent) {
      if (event.key !== "Escape") return;
      setOpen(false);
      buttonRef.current?.focus();
    }
    document.addEventListener("pointerdown", onPointerDown);
    document.addEventListener("keydown", onKeyDown);
    return () => {
      document.removeEventListener("pointerdown", onPointerDown);
      document.removeEventListener("keydown", onKeyDown);
    };
  }, [open]);

  return (
    <div ref={rootRef} className="relative">
      <button
        ref={buttonRef}
        type="button"
        className="download-button"
        aria-expanded={open}
        aria-controls="linux-downloads"
        onClick={() => setOpen((value) => !value)}
      >
        <LinuxIcon className="h-[1.15rem] w-[1.15rem]" />
        Download for Linux
        <ChevronDownIcon className="h-4 w-4 opacity-70" />
      </button>
      {open ? (
        <ul id="linux-downloads" className="download-picker" aria-label="Download for Linux">
          {LINUX_DOWNLOADS.map((option) => {
            const download = previewDownloadById(option.id);
            if (!download) return null;
            return (
              <li key={option.id}>
                <a href={download.href} onClick={() => setOpen(false)}>
                  <span className="text-sm font-medium text-ink">{option.name}</span>
                  <span className="text-xs text-ink-soft">{option.detail}</span>
                  <span className="sr-only"> {download.fileName}</span>
                </a>
              </li>
            );
          })}
        </ul>
      ) : null}
    </div>
  );
}

function FeedbackList({ appMenu }: { appMenu?: "menu bar" | "tray" }) {
  // One row per channel: what you do on the left, where it goes on the right.
  return (
    <section className="mt-10 max-w-sm" aria-labelledby="feedback-heading">
      <h2 id="feedback-heading" className="text-xs text-ink-soft">
        Questions or feedback?
      </h2>
      <ul className="contact-list mt-3">
        <li>
          <a className="contact-row" href={CAPER_INVITE_URL} target="_blank" rel="noreferrer">
            <HashIcon className="h-[15px] w-[15px] shrink-0" />
            <span className="contact-action">Post in #feedback</span>
            <span className="contact-detail">caper.chat</span>
            <ExternalLinkIcon className="contact-trailing" />
          </a>
        </li>
        <li>
          <a className="contact-row" href={X_URL} target="_blank" rel="noreferrer">
            <XIcon className="h-[15px] w-[15px] shrink-0" />
            <span className="contact-action">Message me on X</span>
            <span className="contact-detail">@josevalerio</span>
            <ExternalLinkIcon className="contact-trailing" />
          </a>
        </li>
        <li>
          <CopyEmailRow email={CONTACT_EMAIL} />
        </li>
      </ul>
      {appMenu ? (
        <p className="mt-3 text-xs leading-relaxed text-ink-soft">
          Or choose <span className="text-ink-muted">Send Feedback…</span> from the Captures{" "}
          {appMenu} icon.
        </p>
      ) : null}
    </section>
  );
}

function CopyEmailRow({ email }: { email: string }) {
  const [status, setStatus] = useState<"idle" | "copied" | "failed">("idle");
  const resetTimer = useRef<number | null>(null);

  useEffect(() => {
    return () => {
      if (resetTimer.current !== null) window.clearTimeout(resetTimer.current);
    };
  }, []);

  async function handleCopy() {
    try {
      await navigator.clipboard.writeText(email);
      setStatus("copied");
    } catch {
      setStatus("failed");
    }
    if (resetTimer.current !== null) window.clearTimeout(resetTimer.current);
    resetTimer.current = window.setTimeout(() => setStatus("idle"), 2_500);
  }

  return (
    <>
      <button
        type="button"
        className="contact-row"
        data-copied={status === "copied" ? "" : undefined}
        onClick={() => void handleCopy()}
        title="Copy email address"
      >
        <MailIcon className="h-[15px] w-[15px] shrink-0" />
        <span className="contact-action">Email me</span>
        <span className="contact-detail">{status === "copied" ? "Copied" : email}</span>
        {status === "copied" ? (
          <CheckIcon className="contact-trailing" />
        ) : (
          <CopyIcon className="contact-trailing" />
        )}
      </button>
      <span className={status === "failed" ? "contact-feedback" : "sr-only"} role="status">
        {status === "copied"
          ? "Email address copied to clipboard."
          : status === "failed"
            ? `Couldn’t copy. Email ${email}.`
            : ""}
      </span>
    </>
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
      width="20"
      height="20"
      fill="none"
      stroke="currentColor"
      aria-hidden="true"
      className={className}
    >
      <path d="M9 4H7a3 3 0 0 0-3 3v2M20 15v2a3 3 0 0 1-3 3h-2M9 20H7a3 3 0 0 1-3-3v-2" />
      <circle className="capture-icon-lens" cx="12" cy="12" r="3.4" />
      <circle className="capture-icon-center" cx="12" cy="12" r="1.15" />
      <circle className="capture-icon-shutter" cx="18" cy="6" r="2.4" />
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

function ExternalLinkIcon({ className }: { className?: string }) {
  return (
    <svg
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      className={className}
    >
      <path d="M7 17 17 7M9 7h8v8" />
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

function HashIcon({ className }: { className?: string }) {
  return (
    <svg
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      className={className}
    >
      <path d="M4 9h16M4 15h16M10 3 8 21M16 3l-2 18" />
    </svg>
  );
}

function CopyIcon({ className }: { className?: string }) {
  return (
    <svg
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      className={className}
    >
      <rect x="8" y="8" width="14" height="14" rx="2" />
      <path d="M4 16c-1.1 0-2-.9-2-2V4c0-1.1.9-2 2-2h10c1.1 0 2 .9 2 2" />
    </svg>
  );
}

function CheckIcon({ className }: { className?: string }) {
  return (
    <svg
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      className={className}
    >
      <path d="M20 6 9 17l-5-5" />
    </svg>
  );
}

function ChevronDownIcon({ className }: { className?: string }) {
  return (
    <svg
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      className={className}
    >
      <path d="m6 9 6 6 6-6" />
    </svg>
  );
}
