#!/usr/bin/env node
/**
 * Finds the binary for this machine and gets out of the way.
 *
 * cqx is a Rust program. This package exists so that `npx @cqxai/cli` works and so
 * that a CI job can use it without a Rust toolchain — not to wrap it. The
 * wrapper's whole job is to exec the real thing and return its exit code.
 *
 * The binary arrives as an optional dependency, one package per platform,
 * each declaring the `os` and `cpu` it is for. npm installs only the one that
 * matches, and skips the rest without failing. That is why there is no
 * postinstall script here: nothing is downloaded, nothing is unpacked, and
 * `npm ci --ignore-scripts` — which is how a careful CI installs anything —
 * gets a working tool rather than a broken one.
 */
import { spawnSync } from 'node:child_process';
import { createRequire } from 'node:module';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';

const PLATFORMS = {
  'darwin-arm64': '@cqxai/cqx-darwin-arm64',
  'darwin-x64': '@cqxai/cqx-darwin-x64',
  'linux-x64': '@cqxai/cqx-linux-x64',
  'win32-x64': '@cqxai/cqx-windows-x64',
};

const key = `${process.platform}-${process.arch}`;
const pkg = PLATFORMS[key];

if (!pkg) {
  console.error(
    `cqx: no binary is published for ${key}.\n` +
      `Supported: ${Object.keys(PLATFORMS).join(', ')}.\n` +
      `Building from source works everywhere: cargo install cqx`,
  );
  process.exit(1);
}

/**
 * Where to look from.
 *
 * `import.meta.url` is the right answer for an ordinary install and the
 * wrong one whenever this package is reached through a symlink — which pnpm
 * does for everything, `npm link` does by definition, and a local directory
 * dependency does too. Node resolves the symlink before handing over the
 * URL, so the search starts in the source tree the link points at, where the
 * consumer's node_modules does not exist.
 *
 * So: ask from here, then from whatever invoked us, then from the project
 * being worked in. The first that answers wins.
 */
const from = [
  import.meta.url,
  process.argv[1] ? pathToFileURL(process.argv[1]) : null,
  pathToFileURL(join(process.cwd(), 'package.json')),
].filter(Boolean);

const wanted = `${pkg}/bin/cqx${process.platform === 'win32' ? '.exe' : ''}`;

let binary;
for (const base of from) {
  try {
    binary = createRequire(base).resolve(wanted);
    break;
  } catch {
    // Try the next base.
  }
}

if (!binary) {
  // The platform package is optional, so npm will have carried on without it
  // — a network failure during install, or an --omit=optional that was meant
  // for something else. Naming the package is the difference between a
  // minute and an afternoon.
  console.error(
    `cqx: ${pkg} is not installed, so there is no binary to run.\n` +
      `It is an optional dependency of @cqxai/cli and something skipped it —\n` +
      `a network failure during install, or --omit=optional.\n` +
      `Try: npm install ${pkg}`,
  );
  process.exit(1);
}

// Inherited stdio, so the tool's output is the tool's output: colours,
// progress and a terminal it can detect. `shell: false` so an argument
// containing a space or a quote reaches the program as written.
const { status, signal, error } = spawnSync(binary, process.argv.slice(2), {
  stdio: 'inherit',
  shell: false,
});

if (error) {
  console.error(`cqx: could not run ${binary}: ${error.message}`);
  process.exit(1);
}
// A process that died on a signal did not exit with a code. Reporting 0 there
// would tell a CI job that an interrupted run had succeeded.
process.exit(signal ? 1 : (status ?? 1));
