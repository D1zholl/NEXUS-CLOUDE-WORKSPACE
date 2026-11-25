import { describe, expect, it } from 'vitest';

import {
  basename,
  dirname,
  extensionOf,
  fileKind,
  highlightLanguage,
  joinPath,
  languageForPath,
} from './fileLanguage';

describe('fileLanguage', () => {
  it('extracts extensions and names', () => {
    expect(extensionOf('src/App.tsx')).toBe('tsx');
    expect(extensionOf('README.md')).toBe('md');
    expect(extensionOf('Dockerfile')).toBe('dockerfile');
    expect(basename('src/App.tsx')).toBe('App.tsx');
    expect(dirname('src/lib/a.ts')).toBe('src/lib');
    expect(dirname('a.ts')).toBe('');
    expect(joinPath('', 'a.ts')).toBe('a.ts');
    expect(joinPath('src/', 'a.ts')).toBe('src/a.ts');
  });

  it('classifies file kinds', () => {
    expect(fileKind('logo.PNG')).toBe('image');
    expect(fileKind('README.md')).toBe('markdown');
    expect(fileKind('package.json')).toBe('json');
    expect(fileKind('archive.zip')).toBe('binary');
    expect(fileKind('main.rs')).toBe('text');
  });

  it('maps languages but only highlights registered ones', () => {
    expect(languageForPath('main.rs')).toBe('rust');
    expect(languageForPath('app.tsx')).toBe('typescript');
    expect(languageForPath('style.css')).toBe('css');
    expect(languageForPath('notes.txt')).toBeNull();
    expect(highlightLanguage('main.rs')).toBe('rust');
    // CSS не зарегистрирован в lib/highlight.ts.
    expect(highlightLanguage('style.css')).toBeNull();
  });
});
