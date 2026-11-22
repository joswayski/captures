import assert from "node:assert/strict";
import {
  chmodSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { afterEach, beforeEach, test, vi } from "vitest";
import { fileURLToPath } from "node:url";
import {
  ensureStore,
  restoreSidecars,
  saveSidecars,
  SIDECAR_PATTERNS,
  STORE_NAME,
} from "./media-sidecar-store.mjs";

const API = "https://api.github.com/repos/owner/captures/";
const UPLOADS = "https://uploads.github.com/repos/owner/captures/releases/";
const MAC = `ffmpeg-v1-macOS-ARM64-${"a".repeat(64)}-${"b".repeat(64)}`;
const NEWER_MAC = `ffmpeg-v1-macOS-ARM64-${"c".repeat(64)}-${"b".repeat(64)}`;
const WINDOWS = `ffmpeg-v1-Windows-X64-${"a".repeat(64)}-${"b".repeat(64)}`;

// The parts of GitHub's Releases API the store uses, held in memory.
function fakeGithub() {
  const github = { releases: [], assets: new Map(), uploads: 0, nextId: 1 };
  const json = (value, status = 200) => new Response(JSON.stringify(value), { status });
  vi.stubGlobal("fetch", async (url, options = {}) => {
    const method = options.method ?? "GET";
    if (url.startsWith(UPLOADS)) {
      const [releaseId] = url.slice(UPLOADS.length).split("/");
      const name = new URL(url).searchParams.get("name");
      const data = Buffer.concat(await Array.fromAsync(options.body));
      github.assets.set(github.nextId, { id: github.nextId, releaseId, name, data });
      github.nextId += 1;
      github.uploads += 1;
      return json({}, 201);
    }
    const path = url.slice(API.length);
    if (path.startsWith("releases?")) return json(github.releases);
    if (path === "releases" && method === "POST") {
      const release = { ...JSON.parse(options.body), id: github.nextId };
      github.nextId += 1;
      github.releases.push(release);
      return json(release, 201);
    }
    const asset = path.match(/^releases\/assets\/(\d+)$/u);
    if (asset && method === "DELETE") {
      github.assets.delete(Number(asset[1]));
      return new Response(null, { status: 204 });
    }
    if (asset) return new Response(github.assets.get(Number(asset[1])).data);
    const release = path.match(/^releases\/(\d+)(\/assets)?/u);
    if (release && release[2])
      return json(
        [...github.assets.values()]
          .filter((candidate) => candidate.releaseId === release[1])
          .map(({ id, name }) => ({ id, name })),
      );
    if (release) return json({ upload_url: `${UPLOADS}${release[1]}/assets{?name,label}` });
    return json({ message: `unexpected ${method} ${url}` }, 500);
  });
  return github;
}

function builtTree(suffix = "aarch64-apple-darwin") {
  const root = mkdtempSync(join(tmpdir(), "captures-sidecars-"));
  const files = {
    [`apps/desktop/src-tauri/binaries/ffmpeg-${suffix}`]: "ffmpeg binary",
    [`apps/desktop/src-tauri/binaries/ffprobe-${suffix}`]: "ffprobe binary",
    "apps/desktop/src-tauri/ffmpeg/COPYING.LGPLv2.1": "license",
    "target/ffmpeg-build/ffmpeg-8.1.2.tar.xz": "source",
    "target/ffmpeg-build/ffmpeg-8.1.2.tar.xz.asc": "signature",
    "target/ffmpeg-dist/ffmpeg-8.1.2-NOTICE.md": "notice",
    // Never cached: the extracted source tree and other build output.
    "target/ffmpeg-build/ffmpeg-8.1.2/configure": "extracted source",
  };
  for (const [path, content] of Object.entries(files)) {
    mkdirSync(dirname(join(root, path)), { recursive: true });
    writeFileSync(join(root, path), content);
  }
  chmodSync(join(root, `apps/desktop/src-tauri/binaries/ffmpeg-${suffix}`), 0o755);
  return { root, files };
}

const roots = [];
function tree(...args) {
  const built = builtTree(...args);
  roots.push(built.root);
  return built;
}
function emptyRoot() {
  roots.push(mkdtempSync(join(tmpdir(), "captures-sidecars-")));
  return roots.at(-1);
}

beforeEach(() => {
  vi.stubEnv("GITHUB_REPOSITORY", "owner/captures");
  vi.stubEnv("GH_TOKEN", "token");
});
afterEach(() => {
  vi.unstubAllGlobals();
  vi.unstubAllEnvs();
  for (const root of roots.splice(0)) rmSync(root, { recursive: true, force: true });
});

test("the store archives exactly the files prepare-media caches", () => {
  const action = readFileSync(
    fileURLToPath(new URL("../.github/actions/prepare-media/action.yml", import.meta.url)),
    "utf8",
  );
  const lists = [...action.matchAll(/\n {8}path: \|\n((?: {10}.+\n)+)/gu)].map((match) =>
    match[1]
      .split("\n")
      .map((line) => line.trim())
      .filter(Boolean),
  );
  assert.equal(lists.length, 2, "expected the cache restore and save path lists");
  for (const list of lists) assert.deepEqual(list, SIDECAR_PATTERNS);
});

test("a saved build restores into a clean checkout, executable, without build leftovers", async () => {
  fakeGithub();
  const store = await ensureStore();
  const { root, files } = tree();
  assert.equal(await saveSidecars(store, MAC, root), true);

  const clean = emptyRoot();
  assert.equal(await restoreSidecars(store, MAC, clean), true);
  for (const [path, content] of Object.entries(files)) {
    if (path.endsWith("/configure")) {
      assert.throws(() => statSync(join(clean, path)), /ENOENT/u);
      continue;
    }
    assert.equal(readFileSync(join(clean, path), "utf8"), content);
  }
  if (process.platform !== "win32") {
    const mode = statSync(
      join(clean, "apps/desktop/src-tauri/binaries/ffmpeg-aarch64-apple-darwin"),
    );
    assert.equal(mode.mode & 0o111, 0o111);
  }
  assert.throws(() => statSync(join(clean, "target/media-sidecar-store")), /ENOENT/u);
});

test("the draft store is created once and reused", async () => {
  const github = fakeGithub();
  const store = await ensureStore();
  assert.equal(await ensureStore(), store);
  assert.equal(github.releases.length, 1);
  assert.deepEqual(
    { draft: github.releases[0].draft, name: github.releases[0].name },
    { draft: true, name: STORE_NAME },
  );
});

test("duplicate stores resolve to the oldest, whatever order GitHub lists them in", async () => {
  const github = fakeGithub();
  github.releases.push(
    { id: 9, draft: true, name: STORE_NAME },
    { id: 4, draft: true, name: STORE_NAME },
    { id: 2, draft: false, name: STORE_NAME },
    { id: 1, draft: true, name: "Captures Preview 2026.11.22.1" },
  );
  assert.equal(await ensureStore(), 4);
  github.releases.reverse();
  assert.equal(await ensureStore(), 4);
});

test("a key that was never stored is a miss and leaves the checkout alone", async () => {
  fakeGithub();
  const store = await ensureStore();
  const { root } = tree();
  await saveSidecars(store, MAC, root);
  const clean = emptyRoot();
  assert.equal(await restoreSidecars(store, NEWER_MAC, clean), false);
  assert.throws(() => statSync(join(clean, "apps")), /ENOENT/u);
});

test("saving a stored key uploads nothing, and a new key replaces only its own platform", async () => {
  const github = fakeGithub();
  const store = await ensureStore();
  const mac = tree();
  const windows = tree("x86_64-pc-windows-msvc.exe");
  await saveSidecars(store, MAC, mac.root);
  await saveSidecars(store, WINDOWS, windows.root);
  assert.equal(await saveSidecars(store, MAC, mac.root), false);
  assert.equal(github.uploads, 2);

  assert.equal(await saveSidecars(store, NEWER_MAC, mac.root), true);
  assert.deepEqual(
    [...github.assets.values()].map((asset) => asset.name).sort(),
    [`${NEWER_MAC}.tar.gz`, `${WINDOWS}.tar.gz`].sort(),
  );
});

test("a checkout without both binaries is never stored", async () => {
  const github = fakeGithub();
  const store = await ensureStore();
  const { root } = tree();
  rmSync(join(root, "apps/desktop/src-tauri/binaries/ffprobe-aarch64-apple-darwin"));
  await assert.rejects(saveSidecars(store, MAC, root), /no ffprobe sidecar/u);
  assert.equal(github.uploads, 0);
});
