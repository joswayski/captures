import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  chmodSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { afterEach, test } from "vitest";

import { MAX_MANIFEST_BYTES, manifestText } from "./preview-changelog.mjs";
import {
  FORMAT,
  MAX_OFFERS,
  baseName,
  compareVersions,
  fit,
  generatePatches,
  parseBaseName,
  patchName,
  retainedBases,
  staleBases,
} from "./native-update-patches.mjs";

const ROOT = resolve(fileURLToPath(new URL("..", import.meta.url)));
const SCRIPT = join(ROOT, "scripts/native-update-patches.mjs");
const RETAIN = join(ROOT, "scripts/native-update-retain.sh");
const LINUX = "x86_64-unknown-linux-gnu";
const WINDOWS = "x86_64-pc-windows-msvc";
const VERSION = "2026.10.20";
const PATCH_URL = "https://github.com/joswayski/captures/releases/download/native-test";

// Stands in for native_update_patch with its output shape: the patch size comes
// from the base's contents, and a base holding "broken" fails like a patch that
// does not rebuild the target.
const FAKE_TOOL = `#!/usr/bin/env node
const { createHash } = require("node:crypto");
const { readFileSync, writeFileSync } = require("node:fs");
const args = {};
for (let i = 2; i < process.argv.length; i += 2) args[process.argv[i]] = process.argv[i + 1];
const base = readFileSync(args["--base"], "utf8");
if (base === "broken") { console.error("patch does not rebuild the target"); process.exit(1); }
const patch = Buffer.alloc(Number(base), 112);
writeFileSync(args["--output"], patch, { flag: "wx" });
const archive = readFileSync(args["--target-archive"]);
const hash = (bytes) => createHash("sha256").update(bytes).digest("hex");
console.log(JSON.stringify({
  delta: { format: "capturespatch1", patches: [{ base_version: args["--base-version"],
    url: args["--patch-url"], size: patch.length, sha256: hash(patch) }] },
  target: { version: args["--version"], size: archive.length, sha256: hash(archive) },
  files: { copied: 1, bsdiff: 1, whole: 0 },
  listable: patch.length * 2 < archive.length,
}));
`;

const roots = [];
afterEach(() => {
  for (const root of roots.splice(0)) rmSync(root, { recursive: true, force: true });
});

function sha256(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}

function fixture() {
  const root = mkdtempSync(join(tmpdir(), "captures-native-patches-"));
  roots.push(root);
  const tool = join(root, "native_update_patch");
  writeFileSync(tool, FAKE_TOOL);
  chmodSync(tool, 0o755);
  const bases = join(root, "bases");
  const archives = join(root, "archives");
  mkdirSync(bases);
  mkdirSync(archives);
  const artifacts = {};
  for (const [target, name] of [
    [LINUX, "captures-native-linux.tar.gz"],
    [WINDOWS, "captures-native-windows.zip"],
  ]) {
    const bytes = Buffer.alloc(1000, 120);
    writeFileSync(join(archives, name), bytes);
    artifacts[target] = { url: `${PATCH_URL}/${name}`, size: 1000, sha256: sha256(bytes) };
  }
  const manifest = {
    schema: 2,
    identity: "es.captur.native-development",
    renderer: "wgpu",
    version: VERSION,
    notes: "Fixture.",
    artifacts,
  };
  const base = (version, contents, target = LINUX) =>
    writeFileSync(join(bases, baseName(version, target)), contents);
  return { root, tool, bases, archives, manifest, base, output: join(root, "patches") };
}

function generate({ tool, bases, archives, manifest, output }) {
  return generatePatches({ manifest, tool, bases, archives, output, patchBaseUrl: PATCH_URL });
}

// What patch::Candidate::select deserializes: `Descriptor` and `Offer` deny
// unknown fields, `format` must be capturespatch1, at most MAX_OFFERS offers,
// each from a SemVer base, worthwhile, a credential-free HTTPS URL and a hex SHA-256.
function assertSelectable(delta, fullSize) {
  assert.deepEqual(Object.keys(delta).sort(), ["format", "patches"]);
  assert.equal(delta.format, FORMAT);
  assert.ok(Array.isArray(delta.patches) && delta.patches.length <= MAX_OFFERS);
  for (const offer of delta.patches) {
    assert.deepEqual(Object.keys(offer).sort(), ["base_version", "sha256", "size", "url"]);
    compareVersions(offer.base_version, offer.base_version);
    assert.ok(Number.isSafeInteger(offer.size) && offer.size > 0 && offer.size * 2 < fullSize);
    assert.match(offer.sha256, /^[0-9a-f]{64}$/);
    const url = new URL(offer.url);
    assert.equal(url.protocol, "https:");
    assert.ok(!url.username && !url.password && !url.hash);
  }
}

test("compares versions by SemVer precedence and parses only retained base names", () => {
  assert.equal(compareVersions("2026.10.9", "2026.10.10"), -1);
  assert.equal(compareVersions("2026.10.10-rc.1", "2026.10.10"), -1);
  assert.equal(compareVersions("2026.10.10-rc.2", "2026.10.10-rc.10"), -1);
  assert.equal(compareVersions("2026.10.10+build.1", "2026.10.10"), 0);
  assert.throws(() => compareVersions("2026.10", "2026.10.1"));
  assert.deepEqual(parseBaseName(`2026.9.1--${LINUX}.tar.gz`), {
    version: "2026.9.1",
    target: LINUX,
  });
  assert.deepEqual(parseBaseName(`2026.9.1-rc--1--${WINDOWS}.zip`), {
    version: "2026.9.1-rc--1",
    target: WINDOWS,
  });
  for (const name of [
    `2026.9.1--${LINUX}.zip`,
    "2026.9.1--linux.tar.gz",
    "notes.txt",
    `v1--${LINUX}.tar.gz`,
  ]) {
    assert.equal(parseBaseName(name), null, name);
  }
  assert.equal(
    patchName(LINUX, "2026.9.1", VERSION),
    `captures-native-${LINUX}-2026.9.1-to-${VERSION}.capturespatch`,
  );
});

test("patches come from the newest six earlier builds of each target, whatever their order", () => {
  const versions = [
    "2026.9.2",
    "2026.10.1",
    "2026.8.4",
    "2026.9.10",
    "2026.10.10",
    "2026.10.9",
    "2026.8.30",
  ];
  const names = versions.map((version) => baseName(version, LINUX));
  names.push(
    baseName(VERSION, LINUX),
    baseName("2026.11.0", LINUX),
    baseName("2026.10.19", WINDOWS),
    "README",
  );
  assert.deepEqual(
    retainedBases(names, LINUX, VERSION).map((base) => base.version),
    ["2026.10.10", "2026.10.9", "2026.10.1", "2026.9.10", "2026.9.2", "2026.8.30"],
  );
  // Retention keeps each target's newest six, including builds newer than any release.
  assert.deepEqual(
    staleBases(names),
    ["2026.9.2", "2026.8.4", "2026.8.30"].map((version) => baseName(version, LINUX)),
  );
});

test("lists worthwhile patches newest first and drops failing or oversized ones", () => {
  const context = fixture();
  for (const [version, contents] of [
    ["2026.9.2", "100"],
    ["2026.10.1", "broken"], // The generator fails for this base only.
    ["2026.8.4", "100"], // Seventh newest: never tried.
    ["2026.9.10", "500"], // Exactly half: not strictly smaller.
    ["2026.10.10", "120"],
    ["2026.10.9", "600"], // Larger than half.
    ["2026.8.30", "100"],
    [VERSION, "10"], // A rerun of this release never patches from itself.
  ]) {
    context.base(version, contents);
  }
  context.base("2026.10.19", "50", WINDOWS);
  writeFileSync(join(context.bases, "README"), "not a base");
  const before = structuredClone(context.manifest);
  const updated = generate(context);

  const linux = updated.artifacts[LINUX];
  assert.deepEqual(
    linux.delta.patches.map((offer) => offer.base_version),
    ["2026.10.10", "2026.9.2", "2026.8.30"],
  );
  assertSelectable(linux.delta, linux.size);
  assertSelectable(updated.artifacts[WINDOWS].delta, 1000);
  const file = patchName(LINUX, "2026.10.10", VERSION);
  const bytes = readFileSync(join(context.output, file));
  assert.deepEqual(linux.delta.patches[0], {
    base_version: "2026.10.10",
    url: `${PATCH_URL}/${file}`,
    size: 120,
    sha256: sha256(bytes),
  });
  // Only listed patches remain to upload.
  assert.deepEqual(
    readdirSync(context.output).sort(),
    [
      patchName(LINUX, "2026.10.10", VERSION),
      patchName(LINUX, "2026.8.30", VERSION),
      patchName(LINUX, "2026.9.2", VERSION),
      patchName(WINDOWS, "2026.10.19", VERSION),
    ].sort(),
  );
  // Everything else in the manifest is unchanged; the input is not mutated.
  assert.deepEqual(context.manifest, before);
  const { delta: _linux, ...linuxRest } = linux;
  const { delta: _windows, ...windowsRest } = updated.artifacts[WINDOWS];
  assert.deepEqual(
    { ...updated, artifacts: { [LINUX]: linuxRest, [WINDOWS]: windowsRest } },
    before,
  );
});

test("an archive that does not match the manifest, or no bases, lists no patches", () => {
  const context = fixture();
  context.base("2026.10.10", "100");
  context.manifest.artifacts[LINUX].sha256 = "0".repeat(64);
  context.manifest.artifacts[LINUX].delta = { format: FORMAT, patches: [] };
  const updated = generate(context);
  assert.equal(updated.artifacts[LINUX].delta, undefined);
  assert.deepEqual(readdirSync(context.output), []);

  const empty = fixture();
  rmSync(empty.bases, { recursive: true });
  assert.deepEqual(generate(empty), empty.manifest);
});

test("the 128 KiB budget drops the oldest offers across targets first", () => {
  const output = mkdtempSync(join(tmpdir(), "captures-native-fit-"));
  roots.push(output);
  const offers = (target, versions) =>
    versions.map((version) => {
      const file = patchName(target, version, VERSION);
      writeFileSync(join(output, file), "patch");
      return {
        base_version: version,
        url: `${PATCH_URL}/${file}`,
        size: 5,
        sha256: "a".repeat(64),
      };
    });
  const manifest = {
    schema: 2,
    identity: "es.captur.native-development",
    renderer: "wgpu",
    version: VERSION,
    notes: "",
    artifacts: {
      [LINUX]: {
        url: `${PATCH_URL}/linux.tar.gz`,
        size: 1000,
        sha256: "b".repeat(64),
        delta: { format: FORMAT, patches: offers(LINUX, ["2026.10.10", "2026.10.2", "2026.9.30"]) },
      },
      [WINDOWS]: {
        url: `${PATCH_URL}/windows.zip`,
        size: 1000,
        sha256: "c".repeat(64),
        delta: { format: FORMAT, patches: offers(WINDOWS, ["2026.10.9", "2026.10.1"]) },
      },
    },
  };
  const size = (value) => Buffer.byteLength(manifestText(value));
  // Room for all but the two oldest offers (2026.9.30 Linux, 2026.10.1 Windows).
  const withoutTwo = structuredClone(manifest);
  withoutTwo.artifacts[LINUX].delta.patches.pop();
  withoutTwo.artifacts[WINDOWS].delta.patches.pop();
  manifest.notes = "x".repeat(MAX_MANIFEST_BYTES - size(withoutTwo));
  assert.ok(size(manifest) > MAX_MANIFEST_BYTES);
  const fitted = fit(manifest, output);
  assert.ok(size(fitted) <= MAX_MANIFEST_BYTES);
  assert.deepEqual(
    fitted.artifacts[LINUX].delta.patches.map((offer) => offer.base_version),
    ["2026.10.10", "2026.10.2"],
  );
  assert.deepEqual(
    fitted.artifacts[WINDOWS].delta.patches.map((offer) => offer.base_version),
    ["2026.10.9"],
  );
  assert.ok(!existsSync(join(output, patchName(LINUX, "2026.9.30", VERSION))));
  assert.ok(!existsSync(join(output, patchName(WINDOWS, "2026.10.1", VERSION))));
  assert.ok(existsSync(join(output, patchName(WINDOWS, "2026.10.9", VERSION))));

  // Offers go entirely before failing; a manifest too large without them fails.
  manifest.notes = "x".repeat(MAX_MANIFEST_BYTES);
  assert.throws(() => fit(manifest, output), /larger than 131072 bytes/);
  assert.equal(manifest.artifacts[LINUX].delta, undefined);
  assert.equal(manifest.artifacts[WINDOWS].delta, undefined);
});

test("the CLI warns about a failing base, exits zero and writes the manifest it fitted", () => {
  const context = fixture();
  context.base("2026.10.10", "broken");
  context.base("2026.10.9", "100");
  const manifestPath = join(context.root, "manifest.json");
  writeFileSync(manifestPath, JSON.stringify(context.manifest));
  const result = spawnSync(
    process.execPath,
    [
      SCRIPT,
      "generate",
      "--manifest",
      manifestPath,
      "--tool",
      context.tool,
      "--bases",
      context.bases,
      "--archives",
      context.archives,
      "--output",
      context.output,
      "--patch-base-url",
      PATCH_URL,
    ],
    { encoding: "utf8" },
  );
  assert.equal(result.status, 0, result.stderr);
  assert.match(
    result.stdout,
    /::warning::No x86_64-unknown-linux-gnu patch from 2026\.10\.10: patch does not rebuild/,
  );
  const text = readFileSync(manifestPath, "utf8");
  const written = JSON.parse(text);
  assert.equal(text, manifestText(written));
  assert.deepEqual(
    written.artifacts[LINUX].delta.patches.map((offer) => offer.base_version),
    ["2026.10.9"],
  );
  assert.equal(readdirSync(context.root).filter((name) => name.endsWith("-tmp")).length, 0);

  const http = spawnSync(
    process.execPath,
    [
      SCRIPT,
      "generate",
      ...["--manifest", manifestPath, "--tool", context.tool, "--bases", context.bases],
      ...[
        "--archives",
        context.archives,
        "--output",
        context.output,
        "--patch-base-url",
        "http://example.com",
      ],
    ],
    { encoding: "utf8" },
  );
  assert.equal(http.status, 1);
});

test("retention dry-run only reads the bases release and prints its changes", () => {
  const context = fixture();
  const bin = join(context.root, "bin");
  mkdirSync(bin);
  const log = join(context.root, "gh.log");
  const existing = [
    "2026.10.1",
    "2026.10.2",
    "2026.10.3",
    "2026.10.4",
    "2026.10.5",
    "2026.10.6",
    "2026.9.9",
  ]
    .map((version) => baseName(version, LINUX))
    .concat([baseName("2026.10.6", WINDOWS), "README"]);
  // A fake gh: read-only views answer from fixtures; anything else would mutate.
  writeFileSync(
    join(bin, "gh"),
    `#!/usr/bin/env bash
echo "$*" >> ${JSON.stringify(log)}
case "$1 $2 $3" in
  "release view native-update-bases") printf '%s\\n' ${existing.map((name) => JSON.stringify(name)).join(" ")} ;;
  "release view native-test") printf '%s\\n' old.capturespatch ${patchName(LINUX, "2026.10.6", VERSION)} ;;
  *) echo "unexpected mutating gh call" >&2; exit 1 ;;
esac
`,
  );
  chmodSync(join(bin, "gh"), 0o755);
  context.manifest.artifacts[LINUX].delta = {
    format: FORMAT,
    patches: [
      {
        base_version: "2026.10.6",
        url: `${PATCH_URL}/${patchName(LINUX, "2026.10.6", VERSION)}`,
        size: 1,
        sha256: "a".repeat(64),
      },
    ],
  };
  const manifestPath = join(context.root, "manifest.json");
  writeFileSync(manifestPath, manifestText(context.manifest));
  const env = { ...process.env, PATH: `${bin}:${process.env.PATH}`, RELEASE_TAG: "native-test" };
  const output = execFileSync(
    "bash",
    [RETAIN, "--dry-run", "retain", manifestPath, context.archives],
    {
      env,
      encoding: "utf8",
    },
  );
  const lines = output.trim().split("\n");
  assert.equal(lines[0], "dry-run: gh release delete-asset native-test old.capturespatch -y");
  assert.match(
    lines[1],
    new RegExp(
      `^dry-run: gh release upload native-update-bases --clobber \\S+/${VERSION}--${WINDOWS}\\.zip \\S+/${VERSION}--${LINUX}\\.tar\\.gz$`,
    ),
  );
  assert.deepEqual(lines.slice(2).sort(), [
    `dry-run: gh release delete-asset native-update-bases ${baseName("2026.10.1", LINUX)} -y`,
    `dry-run: gh release delete-asset native-update-bases ${baseName("2026.9.9", LINUX)} -y`,
  ]);
  assert.deepEqual(readFileSync(log, "utf8").trim().split("\n"), [
    'release view native-test --json assets --jq .assets[].name | select(endswith(".capturespatch"))',
    "release view native-update-bases --json assets --jq .assets[].name",
  ]);

  // A missing bases release (the fake gh fails the download) is only a warning.
  const download = spawnSync("bash", [RETAIN, "download", join(context.root, "downloaded")], {
    env,
    encoding: "utf8",
  });
  assert.equal(download.status, 0, download.stderr);
  assert.match(download.stdout, /::warning::No retained native builds/);

  // An archive that is not the manifest's is refused before any gh call.
  writeFileSync(join(context.archives, "captures-native-windows.zip"), "tampered");
  const refused = spawnSync(
    "bash",
    [RETAIN, "--dry-run", "retain", manifestPath, context.archives],
    {
      env,
      encoding: "utf8",
    },
  );
  assert.notEqual(refused.status, 0);
  assert.match(refused.stderr, /does not match the manifest's x86_64-pc-windows-msvc artifact/);
});

// Opt-in: builds the real generator (slow) and patches synthetic package.py
// archives. With CAPTURES_NATIVE_DELTA_PROBE and CAPTURES_NATIVE_DELTA_SIGNING_TEST
// it also stages the newest patch through native_delta_smoke.py.
test.skipIf(!process.env.CAPTURES_NATIVE_PATCH_E2E)(
  "end to end: the real generator's patches are listed and accepted",
  { timeout: 60 * 60 * 1000 },
  () => {
    execFileSync("cargo", ["build", "-p", "captures-app", "--bin", "native_update_patch", "-j2"], {
      cwd: ROOT,
      stdio: "inherit",
    });
    const tool = join(ROOT, "target/debug/native_update_patch");
    const root = mkdtempSync(join(tmpdir(), "captures-native-patches-e2e-"));
    roots.push(root);
    const bases = join(root, "bases");
    const archives = join(root, "archives");
    const target = "2026.10.50";
    const baseVersions = [
      "2026.9.95",
      "2026.9.99",
      "2026.9.93",
      "2026.9.97",
      "2026.9.94",
      "2026.9.98",
      "2026.9.96",
    ];
    // The same package.py staging and archiving the Rust updater fixtures use.
    execFileSync(
      "python3",
      [
        "-I",
        "-c",
        `
import importlib.util, io, pathlib, random, sys, tarfile, tempfile
repo, bases, archives, target = sys.argv[1:5]
spec = importlib.util.spec_from_file_location('package', pathlib.Path(repo) / 'apps/native/package.py')
package = importlib.util.module_from_spec(spec); spec.loader.exec_module(package)
program = bytearray(random.Random(7).randbytes(512 * 1024))
def build(version, destination):
    with tempfile.TemporaryDirectory() as root:
        root = pathlib.Path(root)
        changed = bytearray(program)
        seed = sum(map(int, version.split('.')))
        for offset in range(seed * 97, len(changed), 40961): changed[offset] = seed % 256
        (root / 'binary').write_bytes(changed)
        output = root / 'native linux'
        _, executable = package.stage('linux', root / 'binary', output)
        for name in ('ffmpeg', 'ffprobe'):
            path = executable.parent / 'binaries' / (name + '-x86_64-unknown-linux-gnu')
            path.parent.mkdir(exist_ok=True); path.write_bytes(b'inert tool'); path.chmod(0o755)
        for name in ('LICENSE', 'NOTICE.md'):
            path = output / 'media-licenses/openh264' / name; path.parent.mkdir(parents=True, exist_ok=True); path.write_text('fixture notice')
        for ending in ('.tar.xz', '.tar.xz.asc', '-BUILD_CONFIG.txt', '-COPYING.LGPLv2.1', '-NOTICE.md'):
            path = output / 'media-licenses/ffmpeg' / ('ffmpeg-8.1' + ending); path.parent.mkdir(exist_ok=True); path.write_bytes(b'fixture source or notice')
        package.archive('linux', output, executable, destination, 'x86_64-unknown-linux-gnu')
for version in sys.argv[5:]:
    build(version, pathlib.Path(bases) / f'{version}--x86_64-unknown-linux-gnu.tar.gz')
build(target, pathlib.Path(archives) / 'captures-native-linux.tar.gz')
`,
        ROOT,
        bases,
        archives,
        target,
        ...baseVersions,
      ],
      { stdio: ["ignore", "ignore", "inherit"] },
    );
    const archive = readFileSync(join(archives, "captures-native-linux.tar.gz"));
    // A broken base for one more version must not fail the run.
    writeFileSync(join(bases, baseName("2026.10.1", LINUX)), "not an archive");
    const manifestPath = join(root, "manifest.json");
    writeFileSync(
      manifestPath,
      manifestText({
        schema: 2,
        identity: "es.captur.native-development",
        renderer: "wgpu",
        version: target,
        notes: "Synthetic.",
        artifacts: {
          [LINUX]: {
            url: `${PATCH_URL}/captures-native-linux.tar.gz`,
            size: archive.length,
            sha256: sha256(archive),
          },
        },
      }),
    );
    const output = join(root, "patches");
    const generated = spawnSync(
      process.execPath,
      [
        SCRIPT,
        "generate",
        ...["--manifest", manifestPath, "--tool", tool, "--bases", bases, "--archives", archives],
        ...["--output", output, "--patch-base-url", PATCH_URL],
      ],
      { encoding: "utf8" },
    );
    assert.equal(generated.status, 0, generated.stderr);
    assert.match(
      generated.stdout,
      /::warning::No x86_64-unknown-linux-gnu patch from 2026\.10\.1:/,
    );
    const manifest = JSON.parse(readFileSync(manifestPath, "utf8"));
    const delta = manifest.artifacts[LINUX].delta;
    assertSelectable(delta, archive.length);
    // The six newest bases are tried; 2026.10.1 failed, so five are listed.
    assert.deepEqual(
      delta.patches.map((offer) => offer.base_version),
      ["2026.9.99", "2026.9.98", "2026.9.97", "2026.9.96", "2026.9.95"],
    );
    assert.ok(!existsSync(join(output, patchName(LINUX, "2026.9.94", target))));
    for (const offer of delta.patches) {
      const bytes = readFileSync(join(output, offer.url.split("/").at(-1)));
      assert.equal(bytes.length, offer.size);
      assert.equal(sha256(bytes), offer.sha256);
      assert.equal(bytes.subarray(0, 15).toString(), "CAPTURESPATCH1\n");
    }
    const probe = process.env.CAPTURES_NATIVE_DELTA_PROBE;
    const signing = process.env.CAPTURES_NATIVE_DELTA_SIGNING_TEST;
    if (probe && signing) {
      // The shared Rust client stages this exact patch from the installed
      // 2026.9.99 package (and falls back correctly when it is tampered with).
      const smoke = spawnSync(
        "python3",
        [
          join(ROOT, "apps/native/native_delta_smoke.py"),
          ...["--patch-tool", tool, "--signing-test", signing, "--probe", probe],
          ...["--base-archive", join(bases, baseName("2026.9.99", LINUX))],
          ...["--target-archive", join(archives, "captures-native-linux.tar.gz")],
          ...[
            "--output",
            join(root, "smoke"),
            "--existing-patch",
            join(output, patchName(LINUX, "2026.9.99", target)),
          ],
        ],
        { encoding: "utf8", timeout: 30 * 60 * 1000 },
      );
      assert.equal(smoke.status, 0, smoke.stderr);
      const summary = JSON.parse(readFileSync(join(root, "smoke/result.json"), "utf8"));
      assert.ok(summary.passed && summary.patch_listable);
    }
  },
);
