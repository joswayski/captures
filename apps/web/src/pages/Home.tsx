import { useEffect, useRef, useState } from "react";

const REPO_URL = "https://github.com/joswayski/captures";
const REPO_API = "https://api.github.com/repos/joswayski/captures";
const RELEASES_URL = `${REPO_URL}/releases`;
const X_URL = "https://x.com/josevalerio";
const CONTACT_EMAIL = "contact@josevalerio.com";
const PREVIEW_DOWNLOAD_BASE = `${REPO_URL}/releases/download/preview`;
const PREVIEW_TAG = /^v(\d{4})\.(\d{2})\.(\d{2})\.([1-9]\d?)$/u;
/** How often the homepage re-checks Preview build status (and the API cache TTL). */
const PREVIEW_STATUS_POLL_MS = 10 * 60 * 1_000;

const PREVIEW_DOWNLOADS = [
  {
    platform: "macOS 13+",
    arch: "Apple silicon",
    format: "dmg",
    href: `${PREVIEW_DOWNLOAD_BASE}/Captures-macOS-Apple-Silicon.dmg`,
    fileName: "Captures-macOS-Apple-Silicon.dmg",
  },
  {
    platform: "Windows 11",
    arch: "x64",
    format: "exe",
    href: `${PREVIEW_DOWNLOAD_BASE}/Captures-Windows-x64-setup.exe`,
    fileName: "Captures-Windows-x64-setup.exe",
  },
  {
    platform: "Ubuntu / Debian",
    arch: "x64",
    format: "deb",
    href: `${PREVIEW_DOWNLOAD_BASE}/Captures-Linux-x64.deb`,
    fileName: "Captures-Linux-x64.deb",
  },
  {
    platform: "Other Linux",
    arch: "x64",
    format: "AppImage",
    href: `${PREVIEW_DOWNLOAD_BASE}/Captures-Linux-x64.AppImage`,
    fileName: "Captures-Linux-x64.AppImage",
  },
] as const;

const relativeTimeFormatter = new Intl.RelativeTimeFormat("en", {
  numeric: "always",
});

export default function Home() {
  const [now, setNow] = useState(() => Date.now());
  const cookingShas = useCookingPreviewShas(__LATEST_CHANGES__);

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
            <h2 className="text-base font-medium tracking-tight text-ink sm:text-lg">
              Download Captures{" "}
              <span className="text-xs font-normal text-ink-soft">(experimental)</span>
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
              , or{" "}
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
                      <span className="mt-0.5 block text-xs text-ink-soft">{download.arch}</span>
                    </span>
                    <span className="shrink-0 text-right">
                      <span className="block text-xs font-medium text-ink-muted transition-colors duration-200 ease-out group-hover:text-accent-readable">
                        Download
                      </span>
                      <span className="mt-0.5 block text-xs text-ink-soft">{download.format}</span>
                      <span className="sr-only"> {download.fileName}</span>
                    </span>
                  </a>
                </li>
              ))}
            </ul>
            <p className="mt-2.5 text-right text-xs text-ink-soft">
              …or{" "}
              <a
                href={RELEASES_URL}
                target="_blank"
                rel="noreferrer"
                className="font-medium text-ink-muted no-underline underline-offset-2 transition-colors duration-200 ease-out hover:text-accent-readable hover:underline"
              >
                view all releases on GitHub
              </a>
            </p>
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
            {__LATEST_CHANGES__.map((change) => {
              const cooking = cookingShas.has(change.sha);
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
                        <span className="cooking-emoji" aria-hidden="true">
                          🍳
                        </span>{" "}
                        <span>still cooking</span>
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

/** Commits still waiting on a finished Preview publish (newer than the latest release, or actively building). */
function useCookingPreviewShas(changes: readonly LatestChange[]) {
  const [cookingShas, setCookingShas] = useState<ReadonlySet<string>>(() => new Set());

  useEffect(() => {
    let cancelled = false;

    async function refresh() {
      try {
        const next = await resolveCookingPreviewShas(changes);
        if (!cancelled) setCookingShas(next);
      } catch {
        if (!cancelled) setCookingShas(new Set());
      }
    }

    void refresh();
    const interval = window.setInterval(() => void refresh(), PREVIEW_STATUS_POLL_MS);
    return () => {
      cancelled = true;
      window.clearInterval(interval);
    };
  }, [changes]);

  return cookingShas;
}

type GitHubRelease = {
  draft: boolean;
  prerelease: boolean;
  tag_name: string;
  target_commitish: string;
};

type GitHubWorkflowRun = {
  head_sha: string;
  status: string;
  conclusion: string | null;
};

type GitHubWorkflowRuns = {
  workflow_runs?: GitHubWorkflowRun[];
};

function previewVersion(tag: string) {
  const match = PREVIEW_TAG.exec(tag);
  if (!match) return null;
  return match.slice(1).map(Number);
}

function comparePreviewVersions(left: number[], right: number[]) {
  for (let index = 0; index < left.length; index += 1) {
    const order = left[index] - right[index];
    if (order !== 0) return order;
  }
  return 0;
}

async function githubJson<T>(path: string): Promise<T> {
  const response = await fetch(`${REPO_API}${path}`, {
    headers: {
      Accept: "application/vnd.github+json",
      "X-GitHub-Api-Version": "2022-11-28",
    },
  });
  if (!response.ok) {
    throw new Error(`GitHub request failed (${response.status})`);
  }
  return (await response.json()) as T;
}

async function latestPublishedPreviewCommit(): Promise<string | null> {
  const releases = await githubJson<GitHubRelease[]>("/releases?per_page=40");
  let best: { version: number[]; tag: string; commitish: string } | null = null;

  for (const release of releases) {
    if (release.draft || !release.prerelease) continue;
    const version = previewVersion(release.tag_name);
    if (!version) continue;
    if (!best || comparePreviewVersions(version, best.version) > 0) {
      best = {
        version,
        tag: release.tag_name,
        commitish: release.target_commitish,
      };
    }
  }

  if (!best) return null;
  // Releases usually pin the full SHA; fall back if CI used a branch name.
  if (/^[0-9a-f]{40}$/iu.test(best.commitish)) return best.commitish.toLowerCase();

  try {
    const ref = await githubJson<{ object: { sha: string; type: string } }>(
      `/git/ref/tags/${encodeURIComponent(best.tag)}`,
    );
    if (ref.object.type === "commit") return ref.object.sha.toLowerCase();
    const tagObject = await githubJson<{ object: { sha: string } }>(
      `/git/tags/${ref.object.sha}`,
    );
    return tagObject.object.sha.toLowerCase();
  } catch {
    return null;
  }
}

async function recentReleaseRuns(): Promise<GitHubWorkflowRun[]> {
  try {
    const payload = await githubJson<GitHubWorkflowRuns>(
      "/actions/workflows/release.yml/runs?per_page=30",
    );
    return payload.workflow_runs ?? [];
  } catch {
    // Actions API can be picky unauthenticated; releases alone still cover lagging publishes.
    return [];
  }
}

function releaseRunBuckets(runs: GitHubWorkflowRun[]) {
  const building = new Set<string>();
  const failed = new Set<string>();
  const succeeded = new Set<string>();

  for (const run of runs) {
    const sha = run.head_sha?.toLowerCase();
    if (!sha) continue;

    if (run.status === "in_progress" || run.status === "queued" || run.status === "pending") {
      building.add(sha);
      continue;
    }

    if (run.status !== "completed") continue;

    if (run.conclusion === "success") {
      succeeded.add(sha);
    } else if (
      run.conclusion === "failure" ||
      run.conclusion === "cancelled" ||
      run.conclusion === "timed_out" ||
      run.conclusion === "startup_failure"
    ) {
      // Only mark failed if this SHA never also succeeded (retries).
      if (!succeeded.has(sha)) failed.add(sha);
    }
  }

  // A later success for the same SHA clears failure.
  for (const sha of succeeded) failed.delete(sha);

  return { building, failed, succeeded };
}

type CookingPreviewCache = {
  key: string;
  expiresAt: number;
  value: ReadonlySet<string>;
};

let cookingPreviewCache: CookingPreviewCache | null = null;

async function resolveCookingPreviewShas(
  changes: readonly LatestChange[],
): Promise<Set<string>> {
  if (changes.length === 0) return new Set();

  const cacheKey = changes.map((change) => change.sha).join(",");
  const now = Date.now();
  if (
    cookingPreviewCache &&
    cookingPreviewCache.key === cacheKey &&
    now < cookingPreviewCache.expiresAt
  ) {
    return new Set(cookingPreviewCache.value);
  }

  const [publishedCommit, runs] = await Promise.all([
    latestPublishedPreviewCommit(),
    recentReleaseRuns(),
  ]);
  const { building, failed, succeeded } = releaseRunBuckets(runs);

  const cooking = new Set<string>();
  let seenPublished = publishedCommit === null;

  for (const change of changes) {
    const sha = change.sha.toLowerCase();

    // Never show cooking for finished failures/cancels.
    if (failed.has(sha) && !building.has(sha) && !succeeded.has(sha)) {
      if (publishedCommit && sha === publishedCommit) seenPublished = true;
      continue;
    }

    if (building.has(sha)) {
      cooking.add(change.sha);
    }

    if (!seenPublished) {
      if (publishedCommit && sha === publishedCommit) {
        seenPublished = true;
      } else if (!failed.has(sha)) {
        // Newer than the latest published Preview and not a known failed build.
        cooking.add(change.sha);
      }
    }
  }

  cookingPreviewCache = {
    key: cacheKey,
    expiresAt: now + PREVIEW_STATUS_POLL_MS,
    value: cooking,
  };

  return cooking;
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
