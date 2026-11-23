/**
 * Каноническое дерево инстанса и контракт путей MyLauncher.
 *
 * Путь всегда относителен корня версии Minecraft. Клиент лаунчера принимает
 * только ASCII-пути, ограниченный allowlist расширений и запрещает служебные
 * каталоги. Те же правила проверяются на сервере (`minecraft::validation`);
 * этот модуль даёт мгновенную обратную связь в интерфейсе до отправки запроса.
 */

import type { BuildFileRow } from '../services/types';

export const CANONICAL_ROOT_FOLDERS = ['mods', 'shaderpacks', 'resourcepacks', 'config'] as const;
export const LEGACY_ROOT_FOLDERS = ['scripts', 'overrides', 'other'] as const;
export const FIXED_ROOT_PATHS = ['options.txt', ...CANONICAL_ROOT_FOLDERS] as const;

/** Расширения из политики `safe_repository_path` клиента. */
export const ALLOWED_EXTENSIONS = [
  'jar',
  'zip',
  'json',
  'json5',
  'toml',
  'bak',
  'backup',
  'cfg',
  'conf',
  'properties',
  'txt',
  'md',
  'png',
  'jpg',
  'jpeg',
  'webp',
  'mcmeta',
  'dat',
  'nbt',
  'snbt',
  'mcfunction',
  'vsh',
  'fsh',
  'glsl',
  'lang',
  'yml',
  'yaml',
  'xml',
  'ini',
] as const;

const FORBIDDEN_ROOTS = ['versions', 'assets', 'libraries'];
const FORBIDDEN_LAUNCHER_FILES = [
  '.launcher_meta.json',
  '.launcher_meta.json.tmp',
  '.launcher_meta.json.backup',
];
const WINDOWS_RESERVED = /^(con|prn|aux|nul|com[1-9]|lpt[1-9])(?:\.|$)/i;
// Разрешённые символы сегмента: ASCII-буквы/цифры и пунктуация контракта.
// § разрешён самим клиентом MyLauncher.
const ALLOWED_SEGMENT = /^[A-Za-z0-9 ._\-+@()§]+$/;
const INVALID_WINDOWS = /[<>:"|?*]/;
const DRIVE_PREFIX = /^[A-Za-z]:/;

export type BuildPathKind = 'file' | 'dir';

export interface BuildPathValidation {
  ok: boolean;
  normalized: string;
  errors: string[];
  warnings: string[];
  category: string;
  root: string | null;
  canonical: boolean;
}

export function pathKey(path: string): string {
  return path.normalize('NFC').toLowerCase();
}

/** Приводит разделители к `/`, убирает пустые сегменты и `.`. */
export function normalizeBuildPath(raw: string): string {
  return String(raw ?? '')
    .trim()
    .replace(/\\/g, '/')
    .normalize('NFC')
    .split('/')
    .filter((segment) => segment !== '' && segment !== '.')
    .join('/');
}

export function isFixedRootPath(path: string): boolean {
  const key = pathKey(normalizeBuildPath(path));
  return FIXED_ROOT_PATHS.some((value) => value === key);
}

export function isOptionsPath(path: string): boolean {
  return pathKey(normalizeBuildPath(path)) === 'options.txt';
}

export function extensionOf(path: string): string {
  const filename = path.split('/').pop() ?? path;
  if (filename === '.defaults') return 'defaults';
  const dot = filename.lastIndexOf('.');
  return dot > 0 ? filename.slice(dot + 1).toLowerCase() : '';
}

/** Категория определяется путём; для legacy-категорий сохраняется их имя. */
export function categoryForPath(path: string): string {
  const normalized = normalizeBuildPath(path);
  const first = normalized.split('/')[0]?.toLowerCase() ?? '';
  if (first === 'options.txt') return 'config';
  if ((CANONICAL_ROOT_FOLDERS as readonly string[]).includes(first)) return first;
  if ((LEGACY_ROOT_FOLDERS as readonly string[]).includes(first)) return first;
  const extension = extensionOf(normalized);
  if (extension === 'jar') return 'mods';
  if (extension === 'zip') return 'resourcepacks';
  if (['toml', 'json', 'json5', 'cfg', 'conf', 'properties', 'txt', 'yml', 'yaml', 'ini'].includes(extension)) {
    return 'config';
  }
  return 'other';
}

/** Проверяет путь ровно по контракту лаунчера и каноническому дереву. */
export function validateBuildPath(raw: string, kind: BuildPathKind = 'file'): BuildPathValidation {
  const errors: string[] = [];
  const warnings: string[] = [];
  const trimmed = String(raw ?? '').trim();
  const unified = trimmed.replace(/\\/g, '/');

  if (trimmed.length === 0) {
    errors.push('Путь не может быть пустым.');
  }
  if (Array.from(trimmed).some((character) => {
    const code = character.charCodeAt(0);
    return code < 32 || code === 127;
  })) {
    errors.push('Путь содержит управляющие символы.');
  }
  if (unified.startsWith('/')) {
    errors.push('Абсолютные пути и UNC запрещены: путь должен быть относительным.');
  }
  if (DRIVE_PREFIX.test(trimmed)) {
    errors.push('Путь с буквой диска запрещён.');
  }
  if (unified.split('/').some((segment) => segment === '..')) {
    errors.push('Путь не может содержать переход на уровень выше (..).');
  }

  const segments = unified.split('/').filter((segment) => segment !== '' && segment !== '.');
  const normalized = segments.join('/');
  if (normalized.length > 512) {
    errors.push('Путь не может быть длиннее 512 символов.');
  }
  if (segments.length === 0 && trimmed.length > 0 && errors.length === 0) {
    errors.push('Путь не может быть пустым.');
  }

  for (const segment of segments) {
    if (segment.length > 128) {
      errors.push(`Сегмент «${segment}» длиннее 128 символов.`);
    }
    if (INVALID_WINDOWS.test(segment)) {
      errors.push(`Сегмент «${segment}» содержит символ, недопустимый в путях Windows.`);
    }
    if (segment.endsWith(' ') || segment.endsWith('.')) {
      errors.push(`Сегмент «${segment}» не может заканчиваться точкой или пробелом.`);
    }
    if (WINDOWS_RESERVED.test(segment)) {
      errors.push(`Имя «${segment}» зарезервировано Windows.`);
    }
    if (!ALLOWED_SEGMENT.test(segment)) {
      errors.push(
        `Сегмент «${segment}» содержит недопустимые символы: разрешены только ASCII-буквы, цифры и " _-+@()§".`,
      );
    }
  }

  let finalSegments = [...segments];
  let canonical = false;
  let root: string | null = null;
  const first = segments[0]?.toLowerCase() ?? '';

  if (FORBIDDEN_ROOTS.includes(first)) {
    errors.push(`Каталог «${segments[0]}» зарезервирован лаунчером (versions/assets/libraries).`);
  }
  if (FORBIDDEN_LAUNCHER_FILES.includes(first)) {
    errors.push('Служебные файлы .launcher_meta.json запрещены в сборке.');
  }

  if (first === 'options.txt') {
    canonical = true;
    root = 'options.txt';
    if (segments.length !== 1) {
      errors.push('options.txt допускается только в корне версии Minecraft.');
    }
    if (kind !== 'file') {
      errors.push('options.txt — файл, а не папка.');
    }
    finalSegments = ['options.txt'];
  } else if ((CANONICAL_ROOT_FOLDERS as readonly string[]).includes(first)) {
    canonical = true;
    root = first;
    finalSegments[0] = first;
    const depth = segments.length;

    if (first === 'mods') {
      if (kind === 'file') {
        if (depth !== 2) {
          errors.push('mods — плоский каталог: файл должен лежать прямо в mods/, подпапки запрещены.');
        }
        if (extensionOf(normalized) !== 'jar') {
          errors.push('В mods разрешены только файлы .jar.');
        }
      } else if (depth !== 1) {
        errors.push('mods не поддерживает подпапки. Конфиги складывайте в config/.');
      }
    } else if (first === 'shaderpacks' || first === 'resourcepacks') {
      if (kind === 'file') {
        if (depth !== 2) {
          errors.push(`${first} — плоский каталог: файл должен лежать прямо в ${first}/.`);
        }
        if (extensionOf(normalized) !== 'zip') {
          errors.push(`В ${first} разрешены только .zip.`);
        }
      } else if (depth !== 1) {
        errors.push(`${first} не поддерживает подпапки.`);
      }
    } else if (first === 'config') {
      if (kind === 'file' && depth < 2) {
        errors.push('Файл конфига должен лежать внутри config/ или его подпапки.');
      }
      if (depth < 1) {
        errors.push('Укажите путь внутри config/.');
      }
    }
  } else if ((LEGACY_ROOT_FOLDERS as readonly string[]).includes(first)) {
    root = first;
  } else if (segments.length > 0) {
    errors.push(
      `Корневой каталог «${segments[0]}» не входит в каноническое дерево: допустимы options.txt, mods, shaderpacks, resourcepacks, config.`,
    );
  }

  if (segments.length === 1 && kind === 'file' && first !== 'options.txt') {
    errors.push('В корне версии разрешён только options.txt.');
  }

  const isLegacy = root !== null && (LEGACY_ROOT_FOLDERS as readonly string[]).includes(root);
  if (kind === 'file' && segments.length > 0 && !isLegacy) {
    const extension = extensionOf(normalized);
    if (extension !== 'defaults' && !(ALLOWED_EXTENSIONS as readonly string[]).includes(extension)) {
      errors.push(
        extension
          ? `Расширение .${extension} не разрешено политикой лаунчера.`
          : 'У файла нет расширения, разрешённого политикой лаунчера.',
      );
    }
  }

  const normalizedFinal = finalSegments.join('/');
  return {
    ok: errors.length === 0,
    normalized: normalizedFinal,
    errors: [...new Set(errors)],
    warnings,
    category: categoryForPath(normalizedFinal),
    root,
    canonical,
  };
}

export interface ResolvedDrop {
  sourcePath: string;
  targetPath: string;
  error: string | null;
  ambiguous: string[];
}

/**
 * Маршрутизирует перетаскиваемый файл в канонический путь.
 *
 * Возвращает `ambiguous` для `.zip`: без явной цели нельзя отличить
 * resourcepack от shaderpack, поэтому UI обязан показать предпросмотр выбора.
 */
export function resolveDropTarget(
  fileName: string,
  relativePath = '',
  targetFolder = '',
): ResolvedDrop {
  const sourcePath = normalizeBuildPath(relativePath || fileName);
  const sourceSegments = sourcePath.split('/');
  const relDir = sourceSegments.slice(0, -1).join('/');
  const target = normalizeBuildPath(targetFolder);
  const targetRoot = target.split('/')[0]?.toLowerCase() ?? '';
  const name = fileName || sourceSegments[sourceSegments.length - 1] || '';
  const extension = extensionOf(name);
  const ambiguous: string[] = [];
  let candidate = '';

  const stripLead = (value: string, lead: string): string => {
    const parts = value.split('/').filter(Boolean);
    const index = parts.findIndex((part) => part.toLowerCase() === lead);
    if (index >= 0) parts.splice(0, index + 1);
    return parts.join('/');
  };

  if (pathKey(name) === 'options.txt') {
    candidate = 'options.txt';
  } else if (target) {
    if (targetRoot === 'mods') {
      candidate = `mods/${name}`;
    } else if (targetRoot === 'shaderpacks' || targetRoot === 'resourcepacks') {
      candidate = `${targetRoot}/${name}`;
    } else if (targetRoot === 'config') {
      const dir = stripLead(relDir, 'config');
      candidate = [target, dir, name].filter(Boolean).join('/');
    } else {
      candidate = [target, relDir, name].filter(Boolean).join('/');
    }
  } else {
    if (extension === 'jar') {
      candidate = `mods/${name}`;
    } else if (extension === 'zip') {
      const resourceIndex = relDir.toLowerCase().indexOf('resourcepacks');
      const shaderIndex = relDir.toLowerCase().indexOf('shaderpacks');
      if (shaderIndex >= 0 && (resourceIndex < 0 || shaderIndex < resourceIndex)) {
        candidate = `shaderpacks/${name}`;
      } else if (resourceIndex >= 0) {
        candidate = `resourcepacks/${name}`;
      } else {
        ambiguous.push('resourcepacks', 'shaderpacks');
        candidate = `resourcepacks/${name}`;
      }
    } else {
      const dir = stripLead(relDir, 'config');
      candidate = ['config', dir, name].filter(Boolean).join('/');
    }
  }

  const validation = validateBuildPath(candidate, 'file');
  return {
    sourcePath,
    targetPath: validation.normalized || candidate,
    error: validation.ok ? null : (validation.errors[0] ?? 'Путь не соответствует контракту.'),
    ambiguous,
  };
}

export function nextFreePath(path: string, existing: Set<string>): string {
  if (!existing.has(pathKey(path))) return path;
  const slash = path.lastIndexOf('/');
  const dir = slash >= 0 ? path.slice(0, slash + 1) : '';
  const filename = slash >= 0 ? path.slice(slash + 1) : path;
  const dot = filename.lastIndexOf('.');
  const base = dot > 0 ? filename.slice(0, dot) : filename;
  const extension = dot > 0 ? filename.slice(dot) : '';
  for (let index = 1; index < 1000; index += 1) {
    const candidate = `${dir}${base} (${index})${extension}`;
    if (!existing.has(pathKey(candidate))) return candidate;
  }
  return `${dir}${base} (copy)${extension}`;
}

export interface BuildTreeNode {
  path: string;
  label: string;
  category: string | null;
  fixed: boolean;
  depth: number;
  children: BuildTreeNode[];
  files: BuildFileRow[];
  totalFiles: number;
  totalBytes: number;
}

function makeNode(path: string, fixed = false): BuildTreeNode {
  const normalized = normalizeBuildPath(path);
  return {
    path: normalized,
    label: normalized ? (normalized.split('/').pop() ?? normalized) : 'Корень версии',
    category: normalized ? categoryForPath(normalized) : null,
    fixed,
    depth: normalized ? normalized.split('/').length : 0,
    children: [],
    files: [],
    totalFiles: 0,
    totalBytes: 0,
  };
}

/**
 * Строит дерево: фиксированный скелет всегда виден, даже если файлов ещё нет.
 * Legacy-категории и произвольные папки добавляются как дополнительные узлы.
 */
export function buildBuildTree(rows: BuildFileRow[]): BuildTreeNode {
  const nodes = new Map<string, BuildTreeNode>();
  const ensure = (path: string): BuildTreeNode => {
    const normalized = normalizeBuildPath(path);
    const existing = nodes.get(normalized);
    if (existing) return existing;
    const node = makeNode(normalized);
    nodes.set(normalized, node);
    if (normalized) {
      const parentPath = normalized.split('/').slice(0, -1).join('/');
      ensure(parentPath).children.push(node);
    }
    return node;
  };

  const root = ensure('');
  for (const folder of CANONICAL_ROOT_FOLDERS) {
    ensure(folder).fixed = true;
  }

  for (const row of rows) {
    if (row.kind === 'dir') {
      const node = ensure(row.path);
      if (CANONICAL_ROOT_FOLDERS.includes(node.path as (typeof CANONICAL_ROOT_FOLDERS)[number])) {
        node.fixed = true;
      }
      continue;
    }
    const parentPath = normalizeBuildPath(row.path).split('/').slice(0, -1).join('/');
    ensure(parentPath).files.push(row);
  }

  const order = new Map<string, number>();
  CANONICAL_ROOT_FOLDERS.forEach((folder, index) => order.set(folder, index));

  const aggregate = (node: BuildTreeNode): void => {
    node.totalFiles = node.files.length;
    node.totalBytes = node.files.reduce((sum, file) => sum + (file.sizeBytes ?? 0), 0);
    node.children.sort((left, right) => {
      const leftOrder = order.get(left.path) ?? (left.fixed ? 0 : 100);
      const rightOrder = order.get(right.path) ?? (right.fixed ? 0 : 100);
      if (leftOrder !== rightOrder) return leftOrder - rightOrder;
      return left.label.localeCompare(right.label, 'ru');
    });
    for (const child of node.children) {
      aggregate(child);
      node.totalFiles += child.totalFiles;
      node.totalBytes += child.totalBytes;
    }
  };
  aggregate(root);
  return root;
}

export function directChildFolders(tree: BuildTreeNode, path: string): BuildTreeNode[] {
  const normalized = normalizeBuildPath(path);
  if (!normalized) return tree.children;
  const stack = [tree];
  while (stack.length > 0) {
    const node = stack.pop();
    if (!node) continue;
    if (node.path === normalized) return node.children;
    stack.push(...node.children);
  }
  return [];
}

export function directFiles(tree: BuildTreeNode, path: string): BuildFileRow[] {
  const normalized = normalizeBuildPath(path);
  if (!normalized) return tree.files;
  const stack = [tree];
  while (stack.length > 0) {
    const node = stack.pop();
    if (!node) continue;
    if (node.path === normalized) return node.files;
    stack.push(...node.children);
  }
  return [];
}
