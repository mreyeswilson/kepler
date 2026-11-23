// Tests for check-npm-release-gates.mjs. Run: node --test .github/scripts/
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, mkdirSync, writeFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { spawnSync } from 'node:child_process';

const script = resolve(import.meta.dirname, 'check-npm-release-gates.mjs');

function run(overrides = {}, env = {}) {
  const dir = mkdtempSync(join(tmpdir(), 'kepler-gates-'));
  try {
    const pkg = {
      name: '@kepler/pi',
      version: '1.2.3',
      license: 'MIT',
      scripts: { test: 'node --test' },
      ...overrides,
    };
    writeFileSync(join(dir, 'package.json'), JSON.stringify(pkg));
    mkdirSync(join(dir, 'test'));
    writeFileSync(join(dir, 'test', 'a.test.ts'), '');
    const r = spawnSync(process.execPath, [script], {
      encoding: 'utf8',
      env: {
        PATH: process.env.PATH,
        PKG_DIR: dir,
        EXPECTED_VERSION: '1.2.3',
        GITHUB_REF: 'refs/tags/pi-v1.2.3',
        CONFIRM: 'publish @kepler/pi@1.2.3',
        ...env,
      },
    });
    return { code: r.status, out: r.stdout + r.stderr };
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}

test('passes when every gate holds (MIT license is not blocking)', () => {
  const r = run();
  assert.equal(r.code, 0, r.out);
});

test('license no longer blocks, even when missing or UNLICENSED', () => {
  assert.equal(run({ license: 'UNLICENSED' }).code, 0);
  assert.equal(run({ license: undefined }).code, 0);
});

test('private: true still blocks', () => {
  const r = run({ private: true });
  assert.equal(r.code, 1);
  assert.match(r.out, /"private": true/);
});

test('version 0.0.0 still blocks', () => {
  const r = run({ version: '0.0.0' }, { EXPECTED_VERSION: '0.0.0', GITHUB_REF: 'refs/tags/pi-v0.0.0', CONFIRM: 'publish @kepler/pi@0.0.0' });
  assert.equal(r.code, 1);
  assert.match(r.out, /version is "0\.0\.0"/);
});

test('wrong tag, wrong confirmation and install scripts block', () => {
  assert.equal(run({}, { GITHUB_REF: 'refs/tags/v1.2.3' }).code, 1);
  assert.equal(run({}, { CONFIRM: 'yes' }).code, 1);
  assert.equal(run({ scripts: { test: 'x', postinstall: 'x' } }).code, 1);
});
