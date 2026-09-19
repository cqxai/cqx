/**
 * Turns the binaries a release built into packages npm can serve.
 *
 * Five packages: one per platform holding a single executable, and `cqx-cli`
 * which holds no binary at all and depends on all four as
 * `optionalDependencies`. npm installs the one whose `os` and `cpu` match and
 * silently skips the others, so a Linux machine downloads a Linux binary and
 * nothing else.
 *
 * The alternative — one package with a postinstall script that downloads the
 * right binary — is what most tools do and is worse in the place it matters:
 * it does not work under `npm ci --ignore-scripts`, which is how a careful CI
 * installs anything, and it turns every install into a network request
 * against a host that is not the registry.
 *
 * Versions are pinned exactly. `^` on a platform package would let npm
 * resolve a binary from a different release than the wrapper expects, and the
 * two are one program.
 *
 *   node npm/build.mjs <version> <directory of binaries> [output directory]
 *
 * The output directory is wiped before it is written. It defaults to
 * npm/dist, which is why the test passes its own: a test that rebuilt the
 * shared directory from stub binaries would leave five packages full of
 * nothing behind it, ready to publish.
 */
import { cp, mkdir, readFile, rm, writeFile, chmod } from 'node:fs/promises';
import { join, dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const [version, from, into] = process.argv.slice(2);
if (!version || !from) {
  console.error('usage: node npm/build.mjs <version> <dir of binaries> [output dir]');
  process.exit(2);
}

/** What each platform package declares about the machine it is for. */
const PLATFORMS = [
  { name: 'darwin-arm64', os: 'darwin', cpu: 'arm64', file: 'cqx-darwin-arm64', bin: 'cqx' },
  { name: 'darwin-x64', os: 'darwin', cpu: 'x64', file: 'cqx-darwin-x64', bin: 'cqx' },
  { name: 'linux-x64', os: 'linux', cpu: 'x64', file: 'cqx-linux-x64', bin: 'cqx' },
  { name: 'windows-x64', os: 'win32', cpu: 'x64', file: 'cqx-windows-x64.exe', bin: 'cqx.exe' },
];

const out = into ? resolve(into) : join(root, 'npm', 'dist');
await rm(out, { recursive: true, force: true });

const optional = {};

for (const p of PLATFORMS) {
  const pkg = `@samifouad/cqx-${p.name}`;
  const dir = join(out, 'platform', p.name);
  await mkdir(join(dir, 'bin'), { recursive: true });
  await cp(join(from, p.file), join(dir, 'bin', p.bin));
  // npm preserves the executable bit; without it the wrapper's spawn fails
  // with EACCES on a machine that never ran chmod.
  await chmod(join(dir, 'bin', p.bin), 0o755);

  await writeFile(
    join(dir, 'package.json'),
    JSON.stringify(
      {
        name: pkg,
        version,
        description: `The cqx binary for ${p.os} ${p.cpu}. Installed by cqx-cli; not meant to be depended on directly.`,
        license: 'Apache-2.0',
        repository: { type: 'git', url: 'git+https://github.com/samifouad/cqx.git' },
        homepage: 'https://cqx.bio',
        // What makes npm skip this package on every other machine.
        os: [p.os],
        cpu: [p.cpu],
        files: ['bin'],
        publishConfig: { access: 'public' },
      },
      null,
      2,
    ) + '\n',
  );
  optional[pkg] = version;
  console.log(`  ${pkg}@${version}`);
}

// The wrapper, which is the only name anybody types.
const cli = join(out, 'cqx-cli');
await mkdir(cli, { recursive: true });
await cp(join(root, 'npm', 'cqx-cli', 'bin'), join(cli, 'bin'), { recursive: true });
const manifest = JSON.parse(await readFile(join(root, 'npm', 'cqx-cli', 'package.json'), 'utf8'));
manifest.version = version;
manifest.optionalDependencies = optional;
await writeFile(join(cli, 'package.json'), JSON.stringify(manifest, null, 2) + '\n');
await cp(join(root, 'README.md'), join(cli, 'README.md')).catch(() => {});
console.log(`  cqx-cli@${version} → ${Object.keys(optional).length} platforms`);
