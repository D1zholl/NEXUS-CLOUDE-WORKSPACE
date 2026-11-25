/** Определение типа файла и языка подсветки по пути. */

export type FileKind = 'text' | 'markdown' | 'json' | 'image' | 'binary';

const IMAGE_EXTENSIONS = new Set(['png', 'jpg', 'jpeg', 'gif', 'webp', 'bmp', 'avif', 'ico']);
const BINARY_EXTENSIONS = new Set([
  'zip', 'gz', '7z', 'rar', 'exe', 'dll', 'pdf', 'jar', 'class', 'wasm', 'bin', 'dat',
  'mp3', 'mp4', 'wav', 'ogg', 'woff', 'woff2', 'ttf', 'otf', 'so', 'dylib', 'lock',
]);

const LANGUAGE_BY_EXTENSION: Record<string, string> = {
  rs: 'rust',
  ts: 'typescript',
  tsx: 'typescript',
  js: 'javascript',
  jsx: 'javascript',
  mjs: 'javascript',
  cjs: 'javascript',
  json: 'json',
  jsonc: 'json',
  html: 'xml',
  htm: 'xml',
  xml: 'xml',
  svg: 'xml',
  vue: 'xml',
  css: 'css',
  scss: 'scss',
  less: 'less',
  md: 'markdown',
  markdown: 'markdown',
  toml: 'toml',
  yaml: 'yaml',
  yml: 'yaml',
  py: 'python',
  sql: 'sql',
  sh: 'bash',
  bash: 'bash',
  zsh: 'bash',
  ps1: 'powershell',
  psm1: 'powershell',
  ini: 'ini',
  properties: 'properties',
  java: 'java',
  kt: 'kotlin',
  go: 'go',
  c: 'c',
  h: 'c',
  cpp: 'cpp',
  hpp: 'cpp',
  cs: 'csharp',
  php: 'php',
  rb: 'ruby',
  dockerfile: 'dockerfile',
  graphql: 'graphql',
};

export function extensionOf(path: string): string {
  const name = path.split('/').pop() ?? path;
  const index = name.lastIndexOf('.');
  if (index <= 0) return name.toLowerCase() === 'dockerfile' ? 'dockerfile' : '';
  return name.slice(index + 1).toLowerCase();
}

export function fileKind(path: string): FileKind {
  const extension = extensionOf(path);
  if (IMAGE_EXTENSIONS.has(extension)) return 'image';
  if (BINARY_EXTENSIONS.has(extension)) return 'binary';
  if (extension === 'md' || extension === 'markdown') return 'markdown';
  if (extension === 'json' || extension === 'jsonc') return 'json';
  return 'text';
}

export function languageForPath(path: string): string | null {
  const extension = extensionOf(path);
  return LANGUAGE_BY_EXTENSION[extension] ?? null;
}

export function basename(path: string): string {
  return path.split('/').pop() || path;
}

export function dirname(path: string): string {
  const index = path.lastIndexOf('/');
  return index < 0 ? '' : path.slice(0, index);
}

export function joinPath(dir: string, name: string): string {
  return dir ? `${dir.replace(/\/+$/, '')}/${name}` : name;
}

/** Языки, которые реально зарегистрированы в lib/highlight.ts. */
const HIGHLIGHT_LANGUAGES = new Set([
  'bash', 'ini', 'java', 'javascript', 'json', 'properties', 'rust', 'sql',
  'typescript', 'xml', 'yaml', 'toml',
]);

export function highlightLanguage(path: string): string | null {
  const language = languageForPath(path);
  if (!language) return null;
  return HIGHLIGHT_LANGUAGES.has(language) ? language : null;
}
