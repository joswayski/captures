import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const scriptPath = fileURLToPath(new URL("./wait-preview-queue.sh", import.meta.url));
const workflowPath = fileURLToPath(new URL("../.github/workflows/release.yml", import.meta.url));

function writeMockGh(program) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "wait-preview-queue-"));
  const mockPath = path.join(dir, "gh");
  fs.writeFileSync(mockPath, program, { mode: 0o755 });
  return { dir, mockPath };
}

function runWait(env, mockProgram) {
  const { dir } = writeMockGh(mockProgram);
  return spawnSync("bash", [scriptPath], {
    encoding: "utf8",
    env: {
      PATH: `${dir}${path.delimiter}${process.env.PATH}`,
      GITHUB_REPOSITORY: "joswayski/captures",
      GITHUB_RUN_ID: "200",
      PREVIEW_QUEUE_SLEEP_SECONDS: "0",
      ...env,
    },
  });
}

test("Preview concurrency queues pending main pushes instead of dropping intermediates", () => {
  const workflow = fs.readFileSync(workflowPath, "utf8");
  assert.match(
    workflow,
    /concurrency:\n  group: captures-preview-main\n  cancel-in-progress: false\n  queue: max\n/,
  );
});

test("retries a GitHub 502 instead of failing the Preview queue", () => {
  const stateFile = path.join(os.tmpdir(), `wait-preview-queue-${process.pid}.count`);
  fs.writeFileSync(stateFile, "0");
  const result = runWait(
    {},
    `#!/usr/bin/env bash
set -euo pipefail
count_file=${JSON.stringify(stateFile)}
count="$(cat "$count_file")"
count=$((count + 1))
printf '%s' "$count" > "$count_file"
if [[ "$count" -lt 3 ]]; then
  echo "gh: Server Error (HTTP 502)" >&2
  exit 1
fi
`,
  );
  fs.rmSync(stateFile, { force: true });
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stderr, /GitHub API error \(1\)/);
});

test("waits for an earlier in-progress Preview run, then continues", () => {
  const stateFile = path.join(os.tmpdir(), `wait-preview-queue-blocker-${process.pid}.count`);
  fs.writeFileSync(stateFile, "0");
  const result = runWait(
    {},
    `#!/usr/bin/env bash
set -euo pipefail
count_file=${JSON.stringify(stateFile)}
count="$(cat "$count_file")"
count=$((count + 1))
printf '%s' "$count" > "$count_file"
if [[ "$count" -eq 1 ]]; then
  printf '199'
  exit 0
fi
`,
  );
  fs.rmSync(stateFile, { force: true });
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /Waiting for earlier Preview workflow 199 to finish/);
});

test("rejects a missing run id", () => {
  const result = runWait({ GITHUB_RUN_ID: "" }, "#!/usr/bin/env bash\nexit 0\n");
  assert.equal(result.status, 1);
  assert.match(result.stderr, /GITHUB_RUN_ID must be a numeric workflow run id/);
});
