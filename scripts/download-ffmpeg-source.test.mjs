import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { createHash } from "node:crypto";

import {
  DEFAULT_REPOSITORY,
  downloadPinnedFile,
  ensurePinnedFfmpegSource,
  ffmpegSourceUrls,
  pinnedFfmpegFromScript,
  publishedAssetUrls,
} from "./download-ffmpeg-source.mjs";

const root = fileURLToPath(new URL("..", import.meta.url));
const pinScript = readFileSync(join(root, "scripts/build-ffmpeg-sidecars.sh"), "utf8");
const action = readFileSync(join(root, ".github/actions/prepare-media/action.yml"), "utf8");
const workflow = readFileSync(join(root, ".github/workflows/release.yml"), "utf8");
const helperSource = readFileSync(join(root, "scripts/download-ffmpeg-source.mjs"), "utf8");

function sha256(text) {
  return createHash("sha256").update(text).digest("hex");
}

function okBody(body, status = 200) {
  return {
    ok: status >= 200 && status < 300,
    status,
    arrayBuffer: async () => Buffer.from(body),
    json: async () => JSON.parse(Buffer.from(body).toString("utf8")),
  };
}

test("pins FFmpeg version and SHA-256 from the sidecar build script", () => {
  const pin = pinnedFfmpegFromScript(pinScript);
  assert.equal(pin.version, "8.1.2");
  assert.equal(pin.sha256, "464beb5e7bf0c311e68b45ae2f04e9cc2af88851abb4082231742a74d97b524c");
  assert.equal(pin.archiveName, "ffmpeg-8.1.2.tar.xz");
});

test("GitHub Actions tries published copies before ffmpeg.org", () => {
  const github = `https://github.com/${DEFAULT_REPOSITORY}/releases/download/v2026.09.05.15/ffmpeg-8.1.2.tar.xz`;
  const urls = ffmpegSourceUrls({
    filename: "ffmpeg-8.1.2.tar.xz",
    githubUrls: [github],
    githubActions: true,
  });
  assert.equal(urls[0], github);
  assert.ok(urls.indexOf(github) < urls.indexOf("https://ffmpeg.org/releases/ffmpeg-8.1.2.tar.xz"));
});

test("local downloads try ffmpeg.org first, then a published GitHub copy", () => {
  const github = `https://github.com/${DEFAULT_REPOSITORY}/releases/download/v2026.09.05.15/ffmpeg-8.1.2.tar.xz`;
  const urls = ffmpegSourceUrls({
    filename: "ffmpeg-8.1.2.tar.xz",
    githubUrls: [github],
    githubActions: false,
  });
  assert.equal(urls[0], "https://ffmpeg.org/releases/ffmpeg-8.1.2.tar.xz");
  assert.ok(urls.includes(github));
});

test("published asset lookup ignores drafts and keeps unique download URLs", () => {
  const urls = publishedAssetUrls(
    [
      {
        draft: true,
        assets: [{
          name: "ffmpeg-8.1.2.tar.xz",
          browser_download_url: "https://example.test/draft.tar.xz",
        }],
      },
      {
        draft: false,
        assets: [{
          name: "ffmpeg-8.1.2.tar.xz",
          browser_download_url: "https://example.test/v1/ffmpeg-8.1.2.tar.xz",
        }],
      },
      {
        prerelease: true,
        assets: [{
          name: "ffmpeg-8.1.2.tar.xz",
          browser_download_url: "https://example.test/v1/ffmpeg-8.1.2.tar.xz",
        }],
      },
    ],
    ["ffmpeg-8.1.2.tar.xz"],
  );
  assert.deepEqual(urls["ffmpeg-8.1.2.tar.xz"], [
    "https://example.test/v1/ffmpeg-8.1.2.tar.xz",
  ]);
});

test("download helper fails over after a timeout and verifies the pinned checksum", async () => {
  const directory = mkdtempSync(join(tmpdir(), "captures-ffmpeg-dl-"));
  const dest = join(directory, "ffmpeg-8.1.2.tar.xz");
  const body = "official-ffmpeg-source";
  const digest = sha256(body);
  const calls = [];
  const fetchImpl = async (url) => {
    calls.push(String(url));
    if (String(url).includes("ffmpeg.org")) {
      throw new Error("Failed to connect to ffmpeg.org port 443 after 20000 ms: Timeout was reached");
    }
    return okBody(body);
  };
  const result = await downloadPinnedFile({
    dest,
    urls: [
      "https://ffmpeg.org/releases/ffmpeg-8.1.2.tar.xz",
      "https://github.com/joswayski/captures/releases/download/v2026.09.05.15/ffmpeg-8.1.2.tar.xz",
    ],
    sha256: digest,
    fetchImpl,
    attemptsPerUrl: 2,
    log() {},
  });
  assert.equal(
    result.url,
    "https://github.com/joswayski/captures/releases/download/v2026.09.05.15/ffmpeg-8.1.2.tar.xz",
  );
  assert.equal(readFileSync(dest, "utf8"), body);
  assert.equal(calls.filter((url) => url.includes("ffmpeg.org")).length, 2);
  rmSync(directory, { recursive: true, force: true });
});

test("checksum mismatch skips that URL and tries the next host", async () => {
  const directory = mkdtempSync(join(tmpdir(), "captures-ffmpeg-bad-"));
  const dest = join(directory, "ffmpeg.tar.xz");
  const good = "good-bytes";
  const digest = sha256(good);
  const fetchImpl = async (url) => okBody(String(url).includes("ffmpeg.org") ? "wrong" : good);
  const result = await downloadPinnedFile({
    dest,
    urls: [
      "https://ffmpeg.org/releases/ffmpeg-8.1.2.tar.xz",
      "https://github.com/joswayski/captures/releases/download/preview/ffmpeg-8.1.2.tar.xz",
    ],
    sha256: digest,
    fetchImpl,
    attemptsPerUrl: 2,
    log() {},
  });
  assert.match(result.url, /github\.com/u);
  assert.equal(readFileSync(dest, "utf8"), good);
  rmSync(directory, { recursive: true, force: true });
});

test("ensurePinnedFfmpegSource prefers GitHub copies on Actions when ffmpeg.org is down", async () => {
  const workspace = mkdtempSync(join(tmpdir(), "captures-ffmpeg-root-"));
  const archive = "ffmpeg-source-bytes";
  const signature = "signature-bytes";
  const fakeScript = `FFMPEG_VERSION="8.1.2"\nFFMPEG_SHA256="${sha256(archive)}"\n`;
  mkdirSync(join(workspace, "scripts"), { recursive: true });
  writeFileSync(join(workspace, "scripts/build-ffmpeg-sidecars.sh"), fakeScript);
  const pin = pinnedFfmpegFromScript(fakeScript);
  const githubArchive = `https://github.com/${DEFAULT_REPOSITORY}/releases/download/v2026.09.05.15/${pin.archiveName}`;
  const githubSignature = `${githubArchive}.asc`;
  const calls = [];
  const fetchImpl = async (url) => {
    const target = String(url);
    calls.push(target);
    if (target.includes("/repos/") && target.includes("/releases")) {
      return okBody(JSON.stringify([
        {
          draft: false,
          assets: [
            { name: pin.archiveName, browser_download_url: githubArchive },
            { name: pin.signatureName, browser_download_url: githubSignature },
          ],
        },
      ]));
    }
    if (target.includes("ffmpeg.org")) {
      throw new Error("Failed to connect to ffmpeg.org port 443 after 20000 ms: Timeout was reached");
    }
    return okBody(target.endsWith(".asc") ? signature : archive);
  };

  const result = await ensurePinnedFfmpegSource({
    root: workspace,
    fetchImpl,
    env: {
      GITHUB_ACTIONS: "true",
      GITHUB_REPOSITORY: DEFAULT_REPOSITORY,
      GITHUB_TOKEN: "test-token",
    },
    log() {},
  });

  assert.equal(readFileSync(result.paths.archive, "utf8"), archive);
  assert.equal(readFileSync(result.paths.signature, "utf8"), signature);
  assert.equal(calls[0], `https://api.github.com/repos/${DEFAULT_REPOSITORY}/releases?per_page=100`);
  assert.equal(calls[1], githubArchive);
  assert.ok(!calls.some((url) => url.includes("ffmpeg.org")));
  rmSync(workspace, { recursive: true, force: true });
});

test("sidecar packaging downloads with fallbacks after cache restore", () => {
  assert.doesNotMatch(helperSource, /from\s+["']\.\//u);
  assert.match(pinScript, /download-ffmpeg-source\.mjs" --root "\$ROOT"/u);
  assert.doesNotMatch(pinScript, /curl .*ffmpeg\.org/u);
  assert.match(action, /scripts\/download-ffmpeg-source\.mjs/u);
  assert.match(action, /GITHUB_TOKEN: \$\{\{ github\.token \}\}/u);
  const restore = action.indexOf("uses: actions/cache/restore@v4");
  const download = action.indexOf("Download pinned FFmpeg source");
  const validate = action.indexOf("run: bash scripts/build-ffmpeg-sidecars.sh");
  const save = action.indexOf("uses: actions/cache/save@v4");
  assert.ok(restore > 0 && restore < download && download < validate && validate < save);
  assert.match(
    workflow,
    /sparse-checkout:[\s\S]*?scripts\/download-ffmpeg-source\.mjs/u,
  );
});
