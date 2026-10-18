import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { afterEach, test } from "vitest";
import { aiReleaseNotes } from "./ai-release-notes.mjs";

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
    native ? { "scripts/native-preview-build.mjs": "// native" } : { "README.md": "Tauri" },
    "Base",
  );
  return { cwd, git, commit, base };
}

function reply(highlights, extra = {}) {
  return Response.json({
    type: "message",
    stop_reason: "end_turn",
    content: [{ type: "text", text: JSON.stringify({ highlights }) }],
    ...extra,
  });
}
const fallback = "## What's Changed\n* Original commit notes with links\n";
const env = { ANTHROPIC_API_KEY: "test-key" };
const highlight = (commit) => ({
  title: "Keep your capture handy",
  body: "You can reopen your last screenshot.",
  commits: [commit],
});

test.each([true, false])(
  "summarizes only the pinned %s runtime range, including merge changes",
  async (native) => {
    const f = fixture(native);
    const path = native ? "apps/native/wgpu/src/main.rs" : "apps/desktop/ui/src/App.tsx";
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
        [native ? "apps/desktop/ui/src/App.tsx" : "apps/native/wgpu/src/main.rs"]: "OTHER-RUNTIME",
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
        assert.equal(url, "https://api.anthropic.com/v1/messages");
        assert.equal(options.headers["x-api-key"], "test-key");
        assert.equal(options.headers["anthropic-beta"], "server-side-fallback-2026-07-01");
        request = JSON.parse(options.body);
        return reply([highlight(after)], {
          content: [
            { type: "thinking", thinking: "not release prose" },
            { type: "fallback" },
            { type: "text", text: JSON.stringify({ highlights: [highlight(after)] }) },
          ],
        });
      },
    });
    assert.equal(
      notes,
      "## What's Changed\n* Keep your capture handy — You can reopen your last screenshot.\n",
    );
    assert.equal(request.model, "claude-opus-5-5");
    assert.equal(request.output_config.effort, "high");
    assert.equal(request.max_tokens, 16000);
    assert.equal(request.fallbacks, "default");
    const source = JSON.parse(request.messages[0].content);
    assert.deepEqual(
      source.commits.map(({ commit }) => commit),
      [after],
    );
    assert.match(source.commits[0].diff, /REOPEN-SCREENSHOT/u);
    assert.doesNotMatch(
      request.messages[0].content,
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
    env: { ...env, ANTHROPIC_MODEL: "custom-model", ANTHROPIC_EFFORT: "medium" },
    fetch: async (_, options) => {
      assert.ok(options.body.length <= 400_000);
      const request = JSON.parse(options.body);
      assert.equal(request.model, "custom-model");
      assert.equal(request.output_config.effort, "medium");
      source = JSON.parse(request.messages[0].content);
      return reply([]);
    },
  });
  assert.deepEqual(
    source.commits.map(({ commit }) => commit),
    [first, after],
  );
  assert.ok(source.commits.every(({ diff, diffTruncated }) => diff.length > 0 && diffTruncated));
  assert.equal(notes, "## What's Changed\nNo user-facing desktop changes in this Preview.\n");
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
    async () => reply([good], { stop_reason: "max_tokens" }),
    async () => reply([good], { content: [{ type: "tool_use" }] }),
    async () => reply([{ ...good, commits: [f.base] }]),
    async () => reply([{ ...good, commits: [after, after] }]),
    async () => reply([{ ...good, body: "<script>unsafe</script>" }]),
    async () => reply([{ ...good, body: "Now 50% faster" }]),
    async () => reply([{ ...good, title: "x".repeat(101) }]),
    async () => reply([{ ...good, body: "x".repeat(501) }]),
    async () => reply([{ ...good, extra: "unexpected" }]),
    async () => reply(Array(6).fill(good)),
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
    "## What's Changed\nNo user-facing desktop changes in this Preview.\n",
  );
  assert.equal(await aiReleaseNotes("", after, fallback, options), fallback);
  assert.equal(await aiReleaseNotes(f.base, after, fallback, { ...options, env: {} }), fallback);
  f.commit({ "crates/core/src/lib.rs": "code" }, "Improve capture");
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
