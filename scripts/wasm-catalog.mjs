// Stable routing contract. The build and host share this table.
export const ABI_VERSION = 1;
export const MODULES = {
  core: { languages: ['rust', 'typescript', 'javascript', 'go', 'java', 'kotlin', 'swift', 'zig', 'python', 'php'], extensions: ['rs', 'ts', 'tsx', 'js', 'jsx', 'mjs', 'cjs', 'mts', 'cts', 'go', 'java', 'kt', 'kts', 'swift', 'zig', 'py', 'php'] },
  c: { languages: ['c', 'cpp'], extensions: ['c', 'h', 'cc', 'cpp', 'cxx', 'hpp', 'hh', 'hxx', 'C', 'H'] },
  csharp: { languages: ['csharp'], extensions: ['cs', 'csx'] },
};
export function moduleFor(path) {
  const extension = path.split('/').at(-1).split('.').at(-1);
  return Object.keys(MODULES).find(id => MODULES[id].extensions.includes(extension)) ?? 'core';
}
