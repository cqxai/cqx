/**
 * That the npm packaging delivers a working tool.
 *
 * Not that the packages build — that the thing a person installs then runs,
 * forwards its arguments, and returns the exit code CI depends on. The gate
 * is the whole reason cqx is installed in CI at all, and a wrapper that
 * swallowed a non-zero exit would turn every failing build green.
 *
 * It builds the real packages from a real binary, installs them into a
 * throwaway project the way a consumer would, and drives the result.
 *
 *   node npm/test.mjs [path to a cqx binary]
 */
import { execFileSync, spawnSync } from 'node:child_process';
import { mkdtemp, mkdir, writeFile, cp, rm, chmod } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { existsSync } from 'node:fs';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
let failed = 0;
const check = (what, cond, detail = '') => {
  if (cond) console.log('  ok  ' + what);
  else { failed++; console.error(`FAIL: ${what}${detail ? ' — ' + detail : ''}`); }
};

const real =
  process.argv[2] ??
  ['target/release/cqx', '.target/release/cqx'].map((p) => join(root, p)).find(existsSync);
if (!real) {
  console.error('no cqx binary; run `cargo build --release -p cqx` or pass a path');
  process.exit(2);
}

const version = execFileSync(real, ['--version']).toString().trim().replace(/^cqx /, '');
const work = await mkdtemp(join(tmpdir(), 'cqx-npm-'));

// This machine's platform gets the real binary; the others get a stub, since
// npm will refuse to install them here anyway and the point is to prove the
// right one is chosen.
const here = `${process.platform === 'win32' ? 'windows' : process.platform}-${process.arch}`;
const bins = join(work, 'bins');
await mkdir(bins, { recursive: true });
for (const [name, file] of [
  ['darwin-arm64', 'cqx-darwin-arm64'],
  ['darwin-x64', 'cqx-darwin-x64'],
  ['linux-x64', 'cqx-linux-x64'],
  ['windows-x64', 'cqx-windows-x64.exe'],
]) {
  const to = join(bins, file);
  if (name === here) await cp(real, to);
  else await writeFile(to, '#!/bin/sh\nexit 3\n');
  await chmod(to, 0o755);
}

// Into the scratch directory, never npm/dist: these packages hold stubs for
// every platform but this one, and leaving them where a publish would find
// them is how stub binaries reach the registry.
const built = join(work, 'packages');
execFileSync(process.execPath, [join(root, 'npm/build.mjs'), version, bins, built], { stdio: 'pipe' });
check(`built five packages at ${version}`, existsSync(join(built, 'cli/package.json')));

// Installed the way a consumer installs, and with --ignore-scripts, which is
// how a careful CI does it and the reason this package has no postinstall.
const consumer = join(work, 'consumer');
await mkdir(consumer, { recursive: true });
await writeFile(join(consumer, 'package.json'), '{"name":"c","private":true,"version":"1.0.0"}\n');
execFileSync(
  'npm',
  ['install', '--silent', '--ignore-scripts',
   join(built, 'platform', here),
   join(built, 'cli')],
  { cwd: consumer, stdio: 'pipe' },
);

const cqx = join(consumer, 'node_modules', '.bin', 'cqx');
const run = (...args) => spawnSync(cqx, args, { cwd: consumer, encoding: 'utf8' });

check('it installs and reports its version',
  run('--version').stdout.trim() === `cqx ${version}`,
  run('--version').stdout.trim());

check('--help is forwarded', /usage: cqx/.test(run('--help').stdout));

// The one that matters. A wrapper that loses this turns every failing gate
// into a passing build.
const facts = join(work, 'facts.ndjson');
check('extract runs', run('extract', root, '--out', facts, '--quiet').status === 0);
check('the gate fails when it should', run('score', root, '--facts', facts, '--min-score', '101', '--quiet').status === 1);
check('and passes when it should', run('score', root, '--facts', facts, '--min-score', '50', '--quiet').status === 0);

// An argument that would be mangled by a shell has to arrive intact.
const odd = run('score', root, '--facts', facts, '--config', 'no such file.json');
check('arguments with spaces reach the program', odd.status !== 0 && !/sh:/.test(odd.stderr));

await rm(work, { recursive: true, force: true });
console.log(failed ? `\n${failed} failed` : '\nAll checks passed.');
process.exit(failed ? 1 : 0);
