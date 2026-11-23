// Release gate for the Pi adapter npm package. Exits non-zero (blocking publication) unless EVERY gate holds.
// Usage (from the repo root): node .github/scripts/check-npm-release-gates.mjs
// Env: PKG_DIR (default adapters/pi), CONFIRM, EXPECTED_VERSION, GITHUB_REF (tag refs/tags/pi-v<version>).
// This is intentionally strict: AGENTS.md forbids publishing adapters/pi until the maintainers approve it
// and change the conditions below by hand (see RELEASING.md).
import { existsSync, readdirSync, readFileSync } from 'node:fs';
import { join } from 'node:path';

const dir = process.env.PKG_DIR || 'adapters/pi';
const pkg = JSON.parse(readFileSync(join(dir, 'package.json'), 'utf8'));
const failures = [];
const need = (ok, msg) => {
  if (!ok) failures.push(msg);
};

need(pkg.private !== true, 'package.json has "private": true. Publishing is disabled until maintainers approve and remove it.');
need(
  typeof pkg.license === 'string' && pkg.license !== '' && pkg.license.toUpperCase() !== 'UNLICENSED',
  `license is "${pkg.license}". A real SPDX license must be chosen and approved first.`,
);
need(
  ['LICENSE', 'LICENSE.md', 'LICENSE.txt'].some((f) => existsSync(join(dir, f)) || existsSync(f)),
  'No LICENSE file found in the package directory or repository root.',
);
need(typeof pkg.scripts?.test === 'string' && pkg.scripts.test !== '', 'package.json has no "test" script.');
const testsDir = join(dir, 'test');
const testFiles = existsSync(testsDir) ? readdirSync(testsDir).filter((f) => /\.test\.[cm]?[jt]s$/.test(f)) : [];
need(testFiles.length > 0, 'No test files found (expected test/*.test.ts).');
need(!pkg.scripts?.postinstall && !pkg.scripts?.preinstall && !pkg.scripts?.install, 'Install scripts are forbidden.');
need(!pkg.dependencies || Object.keys(pkg.dependencies).length === 0, 'Runtime dependencies need an explicit review.');
need(typeof pkg.version === 'string' && pkg.version !== '0.0.0', `version is "${pkg.version}"; set a real release version.`);
need(/^@[a-z0-9-~][a-z0-9-._~]*\/[a-z0-9-~][a-z0-9-._~]*$/.test(String(pkg.name)), 'package name must be scoped.');

const expected = process.env.EXPECTED_VERSION || '';
need(expected !== '' && expected === pkg.version, `expected_version "${expected}" does not match package.json version "${pkg.version}".`);
const tag = `refs/tags/pi-v${pkg.version}`;
need(process.env.GITHUB_REF === tag, `workflow must be run from tag ${tag} (got "${process.env.GITHUB_REF || ''}").`);
need(process.env.CONFIRM === `publish ${pkg.name}@${pkg.version}`, `confirmation must be exactly "publish ${pkg.name}@${pkg.version}".`);

if (failures.length > 0) {
  console.error('npm release gates NOT satisfied. Nothing was published.');
  for (const f of failures) console.error(`::error::${f}`);
  process.exit(1);
}
console.log(`All npm release gates satisfied for ${pkg.name}@${pkg.version}.`);
