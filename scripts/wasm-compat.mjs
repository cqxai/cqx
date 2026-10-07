// One transitional release, including idempotent reruns of that release.
import { execFileSync } from 'node:child_process';
import { pathToFileURL } from 'node:url';
import { ABI_VERSION } from './wasm-catalog.mjs';

export function shipLegacy(tag, latest, previousManifest) {
  // An already-published tag keeps its own policy, even after later releases.
  if (previousManifest) {
    if (previousManifest.abi_version !== ABI_VERSION || typeof previousManifest.version !== 'string' || !['core', 'c', 'csharp'].every(id => previousManifest.modules?.[id])) throw Error('invalid previous module manifest');
    return Boolean(previousManifest.deprecated_artifacts?.['cqx_wasm.wasm']);
  }
  if (!latest) return true;
  if (!Array.isArray(latest.assets)) throw Error('latest release has no asset inventory');
  if (!latest.assets.some(asset => asset.name === 'manifest.json')) return true;
  if (latest.tag_name !== tag) return false;
  throw Error('rerun requires the previous module manifest');
}

if (process.argv[1] && pathToFileURL(process.argv[1]).href === import.meta.url) {
  const [tag, repo] = process.argv.slice(2);
  if (!tag || !repo) throw Error('usage: node scripts/wasm-compat.mjs TAG OWNER/REPO');
  // Failure to inspect the existing release must fail the build, never silently
  // guess a compatibility policy. cqx already has published releases.
  const latest = JSON.parse(execFileSync('gh', ['api', `repos/${repo}/releases/latest`], { encoding: 'utf8' }));
  let previous;
  let current = latest.tag_name === tag ? latest : null;
  if (!current) {
    try { current = JSON.parse(execFileSync('gh', ['api', `repos/${repo}/releases/tags/${encodeURIComponent(tag)}`], { encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] })); }
    catch (error) { if (!error.stderr?.includes('(HTTP 404)')) throw error; }
  }
  if (current?.assets.some(asset => asset.name === 'manifest.json')) {
    previous = JSON.parse(execFileSync('gh', ['release', 'download', tag, '--repo', repo, '--pattern', 'manifest.json', '--output', '-'], { encoding: 'utf8' }));
  }
  console.log(shipLegacy(tag, latest, previous));
}
