import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { afterEach, test } from "vitest";
import { aiReleaseNotes } from "./ai-release-notes.mjs";
import { releaseNotesBetween } from "./desktop-release.mjs";

const directories = [];
afterEach(() =>
  directories.splice(0).forEach((directory) => rmSync(directory, { recursive: true, force: true })),
);

function fixture(native = true) {
  const cwd = mkdtempSync(join(tmpdir(), "captures-ai-notes-"));
  directories.push(cwd);
  const git = (...args) =>
    execFileSync("git", args, { cwd, encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] }).trim();
  git("init", "-b", "main");
  git("config", "user.name", "Captures Test");
  git("config", "user.email", "test@example.com");
  const commit = (files, message) => {
    for (const [path, text] of Object.entries(files)) {
      mkdirSync(dirname(join(cwd, path)), { recursive: true });
      writeFileSync(join(cwd, path), text);
    }
    git("add", ".");
    git("commit", "-m", message);
    return git("rev-parse", "HEAD");
  };
  const base = commit(
    native ? { "scripts/native-preview-build.mjs": "// native" } : { "README.md": "Captures" },
    "Base",
  );
  return { cwd, git, commit, base };
}

function reply(
  highlights,
  { parts = [{ text: JSON.stringify({ highlights }) }], finishReason = "STOP" } = {},
) {
  return Response.json({ candidates: [{ content: { role: "model", parts }, finishReason }] });
}
const fallback = "## What's Changed\n* Original commit notes with links\n";
const env = { GEMINI_API_KEY: "test-key" };
const highlight = (commit, platforms = ["macos", "windows", "linux"]) => ({
  title: "Keep your capture handy",
  body: "You can reopen your last screenshot.",
  platforms,
  commits: [commit],
});

test.each([true, false])(
  "summarizes native app changes (%s), including merge changes",
  async (native) => {
    const f = fixture(native);
    const path = "apps/native/gtk/src/app.rs";
    f.commit(
      { "apps/web/src/page.tsx": "WEB-ONLY", "crates/core/tests/secret.rs": "TEST-SECRET" },
      "Improve the site",
    );
    f.git("checkout", "-b", "feature");
    f.commit(
      {
        [path]: "REOPEN-SCREENSHOT",
        "shared/private_key.ts": "PRIVATE-KEY",
        "shared/themes.test.ts": "TEST-ONLY",
        "apps/native/wgpu/src/main.rs": "OTHER-RUNTIME",
      },
      "Reopen your last screenshot",
    );
    f.git("checkout", "main");
    f.git("merge", "--no-ff", "feature", "-m", "Restore the last screenshot");
    const after = f.git("rev-parse", "HEAD");
    f.commit({ [path]: "FUTURE-CHANGE" }, "Later change");
    let request;
    const notes = await aiReleaseNotes(f.base, after, fallback, {
      cwd: f.cwd,
      env,
      fetch: async (url, options) => {
        assert.equal(
          url,
          "https://generativelanguage.googleapis.com/v1beta/models/gemini-4-hydrogen:generateContent",
        );
        assert.equal(options.headers["x-goog-api-key"], "test-key");
        request = JSON.parse(options.body);
        const highlights = [highlight(after, ["linux"])];
        return reply(highlights, {
          parts: [
            { text: "not release prose", thought: true },
            { text: JSON.stringify({ highlights }), thoughtSignature: "x" },
          ],
        });
      },
    });
    // Only the GTK host changed, so only Linux lists the highlight.
    assert.equal(
      notes,
      "## What's Changed\n* **Linux:** Keep your capture handy — You can reopen your last screenshot.\n",
    );
    assert.equal(request.generationConfig.thinkingConfig.thinkingLevel, "MEDIUM");
    assert.equal(request.generationConfig.maxOutputTokens, 32768);
    assert.equal(request.generationConfig.responseMimeType, "application/json");
    assert.equal(request.systemInstruction.parts.length, 1);
    const source = JSON.parse(request.contents[0].parts[0].text);
    assert.equal(source.runtime, "native");
    assert.deepEqual(
      source.commits.map(({ commit }) => commit),
      [after],
    );
    assert.match(source.commits[0].diff, /REOPEN-SCREENSHOT/u);
    assert.deepEqual(source.commits[0].platforms, ["linux"]);
    assert.doesNotMatch(
      request.contents[0].parts[0].text,
      /WEB-ONLY|TEST-SECRET|PRIVATE-KEY|TEST-ONLY|OTHER-RUNTIME|FUTURE-CHANGE/u,
    );
  },
);

test("oversized diffs retain every commit and mark truncation within the request budget", async () => {
  const f = fixture();
  const first = f.commit({ "crates/core/src/lib.rs": "a".repeat(450_000) }, "Improve capture");
  const after = f.commit({ "shared/design.css": "b".repeat(230_000) }, "Improve contrast");
  let source;
  const notes = await aiReleaseNotes(f.base, after, fallback, {
    cwd: f.cwd,
    env: { ...env, GEMINI_MODEL: "custom-model", GEMINI_THINKING_LEVEL: "high" },
    fetch: async (url, options) => {
      assert.ok(options.body.length <= 400_000);
      const request = JSON.parse(options.body);
      assert.match(url, /\/models\/custom-model:generateContent$/u);
      assert.equal(request.generationConfig.thinkingConfig.thinkingLevel, "HIGH");
      source = JSON.parse(request.contents[0].parts[0].text);
      return reply([]);
    },
  });
  assert.deepEqual(
    source.commits.map(({ commit }) => commit),
    [first, after],
  );
  assert.ok(source.commits.every(({ diff, diffTruncated }) => diff.length > 0 && diffTruncated));
  assert.equal(notes, "## What's Changed\nNo user-facing desktop changes in this release.\n");
});

test("invalid or unavailable editorial output preserves exact fallback without leaking errors", async () => {
  const f = fixture();
  const after = f.commit({ "crates/core/src/lib.rs": "capture" }, "Improve capture");
  const good = highlight(after);
  const cases = [
    async () => {
      throw new Error("sensitive provider body");
    },
    async () => new Response("sensitive provider body", { status: 429 }),
    async () => new Response("x".repeat(128 * 1024 + 1)),
    async () => new Response("malformed JSON"),
    async () => reply([good], { finishReason: "MAX_TOKENS" }),
    async () => reply([good], { finishReason: "SAFETY" }),
    async () => reply([good], { parts: [{ text: "secret summary", thought: true }] }),
    async () => Response.json({ promptFeedback: { blockReason: "PROHIBITED_CONTENT" } }),
    async () => reply([good], { parts: [{ functionCall: { name: "run" } }] }),
    async () => reply([{ ...good, commits: [f.base] }]),
    async () => reply([{ ...good, commits: [after, after] }]),
    async () => reply([{ ...good, body: "<script>unsafe</script>" }]),
    async () => reply([{ ...good, body: "Now 50% faster" }]),
    async () => reply([{ ...good, title: "x".repeat(101) }]),
    async () => reply([{ ...good, body: "x".repeat(501) }]),
    async () => reply([{ ...good, extra: "unexpected" }]),
    async () => reply(Array(6).fill(good)),
    async () => reply([{ title: good.title, body: good.body, commits: good.commits }]),
    async () => reply([{ ...good, platforms: [] }]),
    async () => reply([{ ...good, platforms: ["ios"] }]),
    async () => reply([{ ...good, platforms: ["linux", "linux"] }]),
  ];
  for (const fetch of cases) {
    const warnings = [];
    const notes = await aiReleaseNotes(f.base, after, fallback, {
      cwd: f.cwd,
      env,
      fetch,
      warn: (...args) => warnings.push(args),
    });
    assert.equal(notes, fallback);
    assert.deepEqual(warnings, [[]], "no raw provider error or source content reaches diagnostics");
  }
});

test("platform-specific highlights are labelled and limited to the hosts their commits touch", async () => {
  const f = fixture();
  const windows = f.commit(
    { "apps/native/windows/src/preferences.rs": "COPY" },
    "Copy text from Preferences",
  );
  const after = f.commit(
    { "apps/native/gtk/src/tray.rs": "TRAY", "apps/native/macos/Sources/Tray.swift": "TRAY" },
    "Tray follows the theme",
  );
  const notesFor = (highlights) =>
    aiReleaseNotes(f.base, after, fallback, {
      cwd: f.cwd,
      env,
      fetch: async (_, options) => {
        const source = JSON.parse(JSON.parse(options.body).contents[0].parts[0].text);
        assert.deepEqual(
          source.commits.map(({ platforms }) => platforms),
          [["windows"], ["macos", "linux"]],
        );
        return reply(highlights);
      },
      warn: () => {},
    });
  assert.equal(
    await notesFor([
      { ...highlight(windows, ["windows"]), title: "Copy text" },
      { ...highlight(after, ["linux", "macos"]), title: "Tray follows theme" },
    ]),
    "## What's Changed\n* **Windows:** Copy text — You can reopen your last screenshot.\n* **macOS, Linux:** Tray follows theme — You can reopen your last screenshot.\n",
  );
  // A Windows-only commit cannot put a highlight in the Mac app.
  assert.equal(await notesFor([highlight(windows, ["windows", "macos"])]), fallback);
  assert.equal(await notesFor([highlight(windows)]), fallback);
});

test("the egui/wgpu workbench stays out of AI and fallback notes while native hosts are eligible", async () => {
  const f = fixture();
  const native = f.commit(
    { "apps/native/wgpu/src/editor.rs": "WORKBENCH-BRUSH" },
    "Smoother background brushing",
  );
  const options = {
    cwd: f.cwd,
    env,
    fetch: async () => {
      throw new Error("workbench-only range must not call the provider");
    },
    warn: () => assert.fail("workbench-only range must not fall back"),
  };
  assert.equal(
    await aiReleaseNotes(f.base, native, fallback, options),
    "## What's Changed\nNo user-facing desktop changes in this release.\n",
  );
  const shipped = f.commit(
    {
      "crates/captures-app/src/preview.rs": "SHIPPED-STACK",
      "apps/native/windows/src/editor.rs": "SHIPPED-DRAG",
      "apps/native/wgpu/src/editor.rs": "WORKBENCH-BRUSH-UPDATE",
    },
    "Keep your capture handy",
  );
  const notes = await aiReleaseNotes(f.base, shipped, fallback, {
    ...options,
    fetch: async (_, options) => {
      const source = JSON.parse(JSON.parse(options.body).contents[0].parts[0].text);
      assert.deepEqual(
        source.commits.map(({ commit }) => commit),
        [shipped],
      );
      assert.match(source.commits[0].diff, /SHIPPED-STACK/u);
      assert.match(source.commits[0].diff, /SHIPPED-DRAG/u);
      assert.doesNotMatch(source.commits[0].diff, /WORKBENCH-/u);
      // Shared crate changes reach every platform.
      assert.deepEqual(source.commits[0].platforms, ["macos", "windows", "linux"]);
      return reply([highlight(shipped)]);
    },
  });
  assert.match(notes, /Keep your capture handy/u);
  const original = process.cwd();
  try {
    process.chdir(f.cwd);
    const deterministic = releaseNotesBetween(f.base, shipped, "joswayski/captures");
    assert.doesNotMatch(deterministic, /Smoother background brushing/u);
    assert.match(deterministic, /Keep your capture handy/u);
  } finally {
    process.chdir(original);
  }
});

test("quiet ranges skip the API; missing key, missing baseline and excessive metadata fall back", async () => {
  const f = fixture();
  const after = f.commit(
    { "apps/web/src/page.tsx": "web", "crates/core/tests/test.rs": "test" },
    "Web and test changes",
  );
  let calls = 0;
  const options = {
    cwd: f.cwd,
    env,
    fetch: async () => {
      calls++;
      throw new Error("unexpected API request");
    },
    warn: () => {},
  };
  assert.equal(
    await aiReleaseNotes(f.base, after, fallback, options),
    "## What's Changed\nNo user-facing desktop changes in this release.\n",
  );
  assert.equal(await aiReleaseNotes("", after, fallback, options), fallback);
  assert.equal(await aiReleaseNotes(f.base, after, fallback, { ...options, env: {} }), fallback);
  const changed = f.commit({ "crates/core/src/lib.rs": "code" }, "Improve capture");
  for (const setting of [
    { GEMINI_MODEL: "../files/x" },
    { GEMINI_MODEL: "Gemini 4 Neon" },
    { GEMINI_THINKING_LEVEL: "minimal" },
    { GEMINI_THINKING_LEVEL: "max" },
  ])
    assert.equal(
      await aiReleaseNotes(f.base, changed, fallback, { ...options, env: { ...env, ...setting } }),
      fallback,
    );
  // Feed the message over stdin rather than the OS's argument-size limit.
  execFileSync("git", ["commit", "--amend", "-F", "-"], {
    cwd: f.cwd,
    input: "a".repeat(400_001),
    stdio: ["pipe", "ignore", "pipe"],
  });
  assert.equal(
    await aiReleaseNotes(f.base, f.git("rev-parse", "HEAD"), fallback, options),
    fallback,
  );
  assert.equal(calls, 0);
});

test("OpenRouter writes the notes when Gemini fails or answers invalidly", async () => {
  const f = fixture();
  const after = f.commit({ "crates/core/src/lib.rs": "capture" }, "Improve capture");
  const geminiFailures = [
    async () => new Response('{"error":{"status":"RESOURCE_EXHAUSTED"}}', { status: 429 }),
    async () => reply([{ ...highlight(after), commits: [f.base] }]),
  ];
  for (const gemini of geminiFailures) {
    const requests = [];
    const notes = await aiReleaseNotes(f.base, after, fallback, {
      cwd: f.cwd,
      env: { ...env, OPENROUTER_API_KEY: "router-key" },
      fetch: async (url, options) => {
        requests.push(JSON.parse(options.body));
        if (!url.startsWith("https://openrouter.ai/")) return gemini();
        assert.equal(url, "https://openrouter.ai/api/v1/chat/completions");
        assert.equal(options.headers.Authorization, "Bearer router-key");
        const [geminiRequest, request] = requests;
        assert.equal(request.model, "meta/muse-spark-1.3");
        assert.equal(request.messages[1].content, geminiRequest.contents[0].parts[0].text);
        const content = JSON.stringify({ highlights: [highlight(after)] });
        return Response.json({ choices: [{ finish_reason: "stop", message: { content } }] });
      },
      warn: () => assert.fail("the fallback answered"),
    });
    assert.equal(
      notes,
      "## What's Changed\n* Keep your capture handy — You can reopen your last screenshot.\n",
    );
    assert.equal(requests.length, 2);
  }
});
