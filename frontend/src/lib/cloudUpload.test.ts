import { describe, expect, it } from 'vitest';

import {
  FOLDER_UPLOAD_MAX_DEPTH,
  FOLDER_UPLOAD_MAX_FILES,
  validateFolderSelection,
} from './cloudUpload';

/**
 * E5: выбор папки проверяется целиком до постановки в очередь.
 * 2001 файл и 101 уровень вложенности отклоняются, ровно 2000/100 — проходят.
 */

function makeFile(path: string, size = 1): File {
  const name = path.split('/').pop() ?? path;
  const file = new File([new Uint8Array(size)], name, { type: 'text/plain' });
  // webkitRelativePath доступен только у файлов из <input webkitdirectory>.
  Object.defineProperty(file, 'webkitRelativePath', { value: path });
  return file;
}

function pathWithDepth(segments: number): string {
  const parts = ['root'];
  for (let index = 1; index < segments - 1; index += 1) {
    parts.push(`level-${index}`);
  }
  parts.push('file.txt');
  return parts.join('/');
}

describe('validateFolderSelection', () => {
  it('rejects 2001 files without partial upload', () => {
    const files = Array.from({ length: FOLDER_UPLOAD_MAX_FILES + 1 }, (_, index) =>
      makeFile(`root/file-${index}.txt`),
    );

    const result = validateFolderSelection(files);

    expect(result.ok).toBe(false);
    if (!result.ok) {
      expect(result.code).toBe('TOO_MANY_FILES');
      expect(result.message).toContain('2000');
      expect(result.message).toContain('Загрузка не начата');
    }
  });

  it('accepts exactly 2000 files', () => {
    const files = Array.from({ length: FOLDER_UPLOAD_MAX_FILES }, (_, index) =>
      makeFile(`root/file-${index}.txt`),
    );

    const result = validateFolderSelection(files);

    expect(result.ok).toBe(true);
    if (result.ok) {
      expect(result.items).toHaveLength(FOLDER_UPLOAD_MAX_FILES);
      expect(result.rootName).toBe('root');
    }
  });

  it('rejects a path deeper than 100 levels', () => {
    const result = validateFolderSelection([makeFile(pathWithDepth(101))]);

    expect(result.ok).toBe(false);
    if (!result.ok) {
      expect(result.code).toBe('TOO_DEEP');
      expect(result.message).toContain('101');
      expect(result.message).toContain('Загрузка не начата');
    }
  });

  it('accepts a path of exactly 100 levels', () => {
    const result = validateFolderSelection([makeFile(pathWithDepth(100))]);

    expect(result.ok).toBe(true);
    if (result.ok) {
      expect(result.items[0].segments).toHaveLength(FOLDER_UPLOAD_MAX_DEPTH);
      expect(result.items[0].folderSegments).toHaveLength(FOLDER_UPLOAD_MAX_DEPTH - 1);
    }
  });

  it('skips empty files and reports the root folder', () => {
    const result = validateFolderSelection([
      makeFile('Docs/empty.txt', 0),
      makeFile('Docs/ok.txt', 3),
    ]);

    expect(result.ok).toBe(true);
    if (result.ok) {
      expect(result.skippedEmpty).toBe(1);
      expect(result.items).toHaveLength(1);
      expect(result.items[0].file.name).toBe('ok.txt');
      expect(result.rootName).toBe('Docs');
    }
  });
});
