import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../', import.meta.url));
const binary = process.env.COMPOSE_TEST_BINARY || 'docker';
const prefix = process.env.COMPOSE_TEST_BINARY ? [] : ['compose'];
const available = spawnSync(binary, [...prefix, 'version'], { encoding: 'utf8' }).status === 0;

test('Compose keeps local services isolated and routes both storage paths to staging', { skip: !available && 'Docker Compose is not installed' }, () => {
  const scratch = mkdtempSync(path.join(tmpdir(), 'captures-compose-'));
  try {
    const envFile = path.join(scratch, 'empty.env');
    writeFileSync(envFile, '');
    const env = {
      PATH: process.env.PATH, HOME: scratch,
      LOCAL_UID: '501', LOCAL_GID: '20',
      AUTH_SECRET: 'a'.repeat(64), MEDIA_WORKER_SECRET: 'b'.repeat(64),
      AWS_PROFILE: 'test-sso', AWS_REGION: 'us-east-1',
      SES_FROM_ADDRESS: 'test@example.invalid', SES_CONFIGURATION_SET: 'test',
      R2_ACCOUNT_ID: 'c'.repeat(32), R2_ACCESS_KEY_ID: 'fake-key',
      R2_SECRET_ACCESS_KEY: 'fake-secret', CLOUDFLARE_API_TOKEN: 'fake-token',
      // A stale developer environment must not select production storage.
      R2_BUCKET: 'production-captures',
    };
    const args = [...prefix, '--env-file', envFile, '-f', path.join(root, 'compose.yaml'), 'config', '--format', 'json'];
    const result = spawnSync(binary, args, { env, encoding: 'utf8' });
    assert.equal(result.status, 0, result.stderr);
    const { services } = JSON.parse(result.stdout);
    assert.deepEqual(Object.keys(services).sort(), ['api', 'db', 'web', 'worker']);
    assert.equal(services.web.ports[0].host_ip, '127.0.0.1');
    assert.equal(services.web.ports[0].target, 5174);
    assert.equal(services.web.ports.length, 1);
    for (const name of ['api', 'db', 'worker']) {
      assert.equal(services[name].network_mode, 'service:web');
      assert.equal(services[name].ports, undefined);
    }
    assert.equal(services.api.environment.CAPTURES_API_BIND, '127.0.0.1:3001');
    assert.equal(services.api.environment.AUTH_ALLOWED_ORIGIN, 'http://localhost:5174');
    assert.equal(services.api.environment.DATABASE_URL, services.api.environment.MIGRATION_DATABASE_URL);
    assert.equal(new URL(services.api.environment.DATABASE_URL).hostname, '127.0.0.1');
    assert.equal(services.api.environment.R2_BUCKET, 'staging-captures');
    assert.equal(services.api.user, '501:20');
    assert.equal(services.api.volumes[0].read_only, true);
    assert.equal(services.api.volumes[0].target, '/home/dev/.aws');
    assert.equal(services.api.depends_on.db.condition, 'service_healthy');
    assert.equal(services.api.environment.MEDIA_WORKER_SECRET, services.worker.environment.MEDIA_WORKER_SECRET);
    assert.match(services.worker.command[2], /\$MEDIA_WORKER_SECRET/);
    for (const secret of ['AUTH_SECRET', 'DATABASE_URL', 'R2_SECRET_ACCESS_KEY', 'CLOUDFLARE_API_TOKEN']) {
      assert.equal(services.web.environment[secret], undefined);
    }
    assert.equal(services.worker.environment.DATABASE_URL, undefined);
    assert.equal(services.worker.environment.R2_SECRET_ACCESS_KEY, undefined);
    const worker = JSON.parse(readFileSync(path.join(root, 'apps/media-worker/wrangler.compose.jsonc'), 'utf8'));
    assert.deepEqual(worker.r2_buckets, [{ binding: 'ASSETS', bucket_name: 'staging-captures', remote: true }]);
    const missing = spawnSync(binary, args, { env: { ...env, MEDIA_WORKER_SECRET: '' }, encoding: 'utf8' });
    assert.notEqual(missing.status, 0);
    assert.match(missing.stderr, /MEDIA_WORKER_SECRET/);
  } finally {
    rmSync(scratch, { recursive: true, force: true });
  }
});
