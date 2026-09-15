import assert from "node:assert/strict";
import test from "node:test";
import { resolveCookingPreviewShas } from "./cookingPreview.ts";

test("homepage publishing status follows native releases, not a newer Tauri release", async (t) => {
  const original = globalThis.fetch;
  t.after(() => { globalThis.fetch = original; });
  const requests: string[] = [];
  const pending = "a".repeat(40);
  const published = "b".repeat(40);
  globalThis.fetch = async (input) => {
    const url = String(input);
    requests.push(url);
    const body = url.includes("/actions/") ? { workflow_runs: [] } : [
      { draft: false, prerelease: true, tag_name: "v2026.09.15.1", target_commitish: pending },
      { draft: false, prerelease: true, tag_name: "native-v2026.09.14.1", target_commitish: published },
    ];
    return new Response(JSON.stringify(body), { status: 200 });
  };
  const now = new Date().toISOString();
  assert.deepEqual(await resolveCookingPreviewShas([
    { sha: pending, committedAt: now },
    { sha: published, committedAt: now },
  ]), [pending]);
  assert.ok(requests.some((url) => url.includes("/workflows/native-release.yml/")));
});
