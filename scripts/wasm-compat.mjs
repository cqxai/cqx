// One transitional release in tag order, including idempotent historical reruns.
import { execFileSync } from 'node:child_process';
import { pathToFileURL } from 'node:url';
import { ABI_VERSION } from './wasm-catalog.mjs';

function version(tag) {
  const match = /^v?(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?$/.exec(tag);
  if (!match) return null;
  const pre = match[4]?.split('.') ?? [];
  if (pre.some(part => /^\d+$/.test(part) && part.length > 1 && part.startsWith('0'))) return null;
  return { numbers: match.slice(1, 4).map(BigInt), pre };
}
function compare(a, b) {
  const order = (x, y) => x < y ? -1 : x > y ? 1 : 0;
  for (let i = 0; i < 3; i++) {
    const result = order(a.numbers[i], b.numbers[i]);
    if (result) return result;
  }
  if (!a.pre.length || !b.pre.length) return order(Boolean(b.pre.length), Boolean(a.pre.length));
  for (let i = 0; i < Math.max(a.pre.length, b.pre.length); i++) {
    if (a.pre[i] === undefined) return -1;
    if (b.pre[i] === undefined) return 1;
    const [left, right] = [a.pre[i], b.pre[i]], numeric = /^\d+$/;
    const [ln, rn] = [numeric.test(left), numeric.test(right)];
    const result = ln && rn ? order(BigInt(left), BigInt(right)) : ln !== rn ? (ln ? -1 : 1) : order(left, right);
    if (result) return result;
  }
  return 0;
}
function inventory(releases) {
  if (!Array.isArray(releases) || releases.some(release => typeof release.tag_name !== 'string' || !Array.isArray(release.assets))) throw Error('release list has no complete tag/asset inventory');
  return releases;
}
const modular = release => release.assets.some(asset => asset.name === 'manifest.json');

export function shipLegacy(tag, releases, previousManifest) {
  const currentVersion = version(tag);
  if (!currentVersion) throw Error(`invalid release version tag: ${tag}`);
  inventory(releases);
  // An already-published tag keeps its own policy, even after later releases.
  if (previousManifest) {
    if (previousManifest.abi_version !== ABI_VERSION || typeof previousManifest.version !== 'string' || !['core', 'c', 'csharp'].every(id => previousManifest.modules?.[id])) throw Error('invalid previous module manifest');
    return Boolean(previousManifest.deprecated_artifacts?.['cqx_wasm.wasm']);
  }
  if (releases.some(release => release.tag_name === tag && modular(release))) throw Error('rerun requires the previous module manifest');
  // Consider every lower version, including prereleases and drafts. Higher tags
  // cannot remove compatibility from an older hotfix or out-of-order release.
  return !releases.some(release => {
    const prior = version(release.tag_name);
    return prior && compare(prior, currentVersion) < 0 && modular(release);
  });
}

if (process.argv[1] && pathToFileURL(process.argv[1]).href === import.meta.url) {
  const [tag, repo] = process.argv.slice(2);
  if (!tag || !repo) throw Error('usage: node scripts/wasm-compat.mjs TAG OWNER/REPO');
  // Fetch all pages; /latest hides prereleases/drafts and is not version order.
  // Inventory failures fail the build instead of guessing a release policy.
  const pages = JSON.parse(execFileSync('gh', ['api', `repos/${repo}/releases`, '--paginate', '--slurp'], { encoding: 'utf8' }));
  if (!Array.isArray(pages) || pages.some(page => !Array.isArray(page))) throw Error('invalid paginated release inventory');
  const releases = inventory(pages.flat());
  const current = releases.find(release => release.tag_name === tag);
  let previous;
  if (current && modular(current)) {
    previous = JSON.parse(execFileSync('gh', ['release', 'download', tag, '--repo', repo, '--pattern', 'manifest.json', '--output', '-'], { encoding: 'utf8' }));
  }
  console.log(shipLegacy(tag, releases, previous));
}
