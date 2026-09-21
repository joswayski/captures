import assert from 'node:assert/strict'
import { mkdtempSync, readFileSync, writeFileSync, rmSync, existsSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { spawnSync } from 'node:child_process'
import test from 'node:test'

for (const [file, component, slug] of [
  ['aws-image.yml', 'web', 'captures'],
  ['aws-api-image.yml', 'API', 'captures-api'],
]) {
  test(`${component} notification targets its own exact-image deployment`, () => {
    const workflow = readFileSync(new URL(`../.github/workflows/${file}`, import.meta.url), 'utf8')
    const step = workflow.split('      - name: Notify deployment webhook\n')[1]
    assert.ok(step, 'publisher must notify Discord')
    assert.match(step, /IMAGE_DIGEST: \$\{\{ steps\.record\.outputs\.digest \}\}/)
    const url = step.match(/DEPLOY_WORKFLOW_URL: (\S+)/)[1]
    assert.equal(url, `https://github.com/joswayski/infrastructure/actions/workflows/deploy-${slug}.yml`)
    const script = step.split('        run: |\n')[1].replace(/^          /gm, '')
    const dir = mkdtempSync(join(tmpdir(), 'captures-notify-'))
    try {
      writeFileSync(join(dir, 'curl'), `#!/bin/bash
if [[ "$*" == *'/pulls?'* ]]; then
  printf '%s' '[{"title":"Fix connection","body":"Account API fix"}]'
  exit 0
fi
while (( $# )); do
  if [[ "$1" == --data ]]; then printf '%s' "$2" > "$PAYLOAD_FILE"; shift; fi
  shift
done
`, { mode: 0o755 })
      const sha = 'a'.repeat(40)
      const digest = `sha256:${'b'.repeat(64)}`
      const env = {
        ...process.env, PATH: `${dir}:${process.env.PATH}`,
        PAYLOAD_FILE: join(dir, 'payload.json'),
        GITHUB_SHA: sha, IMAGE_DIGEST: digest,
        GITHUB_TOKEN: 'fake', GITHUB_API_URL: 'https://github.invalid',
        GITHUB_REPOSITORY: 'joswayski/captures',
        COMMIT_MESSAGE: 'Fallback title\nFallback body',
        DEPLOY_WORKFLOW_URL: url, DEPLOY_NOTIFICATION_WEBHOOK_URL: 'https://discord.invalid/test',
      }
      const result = spawnSync('bash', ['-c', script], { env, encoding: 'utf8' })
      assert.equal(result.status, 0, result.stderr)
      const payload = JSON.parse(readFileSync(env.PAYLOAD_FILE, 'utf8'))
      assert.equal(payload.embeds[0].author.name, `Captures ${component} image is ready`)
      assert.equal(payload.embeds[0].title, 'Fix connection')
      assert.deepEqual(
        payload.embeds[0].fields.map(({ name }) => name),
        ['Description', 'Git SHA', 'Digest'],
      )
      const [deploy, github] = payload.components[0].components
      assert.equal(deploy.label, `Deploy Captures ${component}`)
      assert.equal(deploy.custom_id, `production-deploy:v1:${slug}:${sha}`)
      assert.equal(github.url, url)
      assert.equal(payload.embeds[0].fields.find(f => f.name === 'Digest').value, `\`${digest}\``)
      rmSync(env.PAYLOAD_FILE)
      const skipped = spawnSync('bash', ['-c', script], {
        env: { ...env, DEPLOY_NOTIFICATION_WEBHOOK_URL: '' }, encoding: 'utf8',
      })
      assert.equal(skipped.status, 0, skipped.stderr)
      assert.equal(existsSync(env.PAYLOAD_FILE), false)
    } finally {
      rmSync(dir, { recursive: true, force: true })
    }
  })
}
