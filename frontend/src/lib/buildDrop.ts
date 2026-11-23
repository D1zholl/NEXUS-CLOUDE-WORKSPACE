/**
 * Рекурсивный обход Drag & Drop из проводника.
 *
 * `DataTransferItem.webkitGetAsEntry()` — единственный способ получить
 * структуру папки. Обход ограничен по глубине и числу файлов, каталоги
 * читаются порциями (Chrome отдаёт не больше 100 записей за вызов).
 */

export interface DroppedFile {
  file: File;
  /** Путь внутри перетаскиваемой папки, с прямыми `/`. */
  relativePath: string;
}

export interface CollectOptions {
  maxFiles?: number;
  maxDepth?: number;
  concurrency?: number;
}

const DEFAULT_MAX_FILES = 5000;
const DEFAULT_MAX_DEPTH = 32;
const DEFAULT_CONCURRENCY = 4;

function readAllEntries(reader: FileSystemDirectoryReader): Promise<FileSystemEntry[]> {
  return new Promise((resolve, reject) => {
    const entries: FileSystemEntry[] = [];
    const read = (): void => {
      reader.readEntries(
        (batch) => {
          if (batch.length === 0) {
            resolve(entries);
            return;
          }
          entries.push(...batch);
          read();
        },
        (error) => reject(error),
      );
    };
    read();
  });
}

function entryFile(entry: FileSystemFileEntry): Promise<File> {
  return new Promise((resolve, reject) => {
    entry.file(resolve, reject);
  });
}

async function walkEntry(
  entry: FileSystemEntry,
  prefix: string,
  state: { count: number; maxFiles: number; maxDepth: number },
  depth: number,
): Promise<DroppedFile[]> {
  const path = prefix ? `${prefix}/${entry.name}` : entry.name;
  if (depth > state.maxDepth) return [];
  if (entry.isFile) {
    if (state.count >= state.maxFiles) return [];
    state.count += 1;
    try {
      const file = await entryFile(entry as FileSystemFileEntry);
      return [{ file, relativePath: path }];
    } catch {
      return [];
    }
  }
  if (!entry.isDirectory) return [];
  const directory = entry as FileSystemDirectoryEntry;
  let entries: FileSystemEntry[] = [];
  try {
    entries = await readAllEntries(directory.createReader());
  } catch {
    return [];
  }
  const collected: DroppedFile[] = [];
  for (const child of entries) {
    if (state.count >= state.maxFiles) break;
    const nested = await walkEntry(child, path, state, depth + 1);
    collected.push(...nested);
  }
  return collected;
}

export async function collectDroppedFiles(
  transfer: Pick<DataTransfer, 'items' | 'files'>,
  options: CollectOptions = {},
): Promise<DroppedFile[]> {
  const maxFiles = options.maxFiles ?? DEFAULT_MAX_FILES;
  const maxDepth = options.maxDepth ?? DEFAULT_MAX_DEPTH;
  const concurrency = Math.max(1, options.concurrency ?? DEFAULT_CONCURRENCY);
  const state = { count: 0, maxFiles, maxDepth };

  const roots: FileSystemEntry[] = [];
  for (const item of Array.from(transfer.items ?? [])) {
    if (item.kind !== 'file') continue;
    const entry = item.webkitGetAsEntry?.();
    if (entry) roots.push(entry);
  }

  if (roots.length === 0) {
    // Firefox/старые браузеры и тесты: структуры папки нет, но файлы есть.
    return Array.from(transfer.files ?? []).slice(0, maxFiles).map((file) => ({
      file,
      relativePath:
        (file as File & { webkitRelativePath?: string }).webkitRelativePath || file.name,
    }));
  }

  const results: DroppedFile[] = [];
  let cursor = 0;
  const workers = Array.from({ length: Math.min(concurrency, roots.length) }, async () => {
    while (cursor < roots.length) {
      const index = cursor;
      cursor += 1;
      const entry = roots[index];
      if (!entry) continue;
      const files = await walkEntry(entry, '', state, 0);
      results.push(...files);
    }
  });
  await Promise.all(workers);
  return results;
}
