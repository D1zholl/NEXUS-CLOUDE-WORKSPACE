import { describe, expect, it } from 'vitest';

import {
  PREVIEW_MAX_BYTES,
  PREVIEW_TEXT_MAX_BYTES,
  canPreviewFile,
  languageForFile,
  previewLimitWarning,
  selectViewer,
} from './preview';

/** Выбор viewer по типу файла и лимиты предпросмотра. */
describe('preview viewer selection', () => {
  it('chooses a viewer by file type', () => {
    expect(selectViewer('document.pdf')).toBe('pdf');
    expect(selectViewer('photo.PNG')).toBe('image');
    expect(selectViewer('clip.webm')).toBe('video');
    expect(selectViewer('track.flac')).toBe('audio');
    expect(selectViewer('app.js')).toBe('code');
    expect(selectViewer('config.yaml')).toBe('code');
    expect(selectViewer('README.md')).toBe('text');
    expect(selectViewer('notes.txt')).toBe('text');
  });

  it('does not preview executables, archives or HTML/SVG', () => {
    expect(selectViewer('setup.exe')).toBe('none');
    expect(selectViewer('archive.zip')).toBe('none');
    expect(selectViewer('page.html')).toBe('none');
    expect(selectViewer('icon.svg')).toBe('none');
    expect(canPreviewFile('folder', 'document.pdf')).toBe(false);
    expect(canPreviewFile('file', 'app.ts')).toBe(true);
  });

  it('maps code extensions to highlight.js languages', () => {
    expect(languageForFile('data.json')).toBe('json');
    expect(languageForFile('main.ts')).toBe('typescript');
    expect(languageForFile('deploy.sh')).toBe('bash');
    expect(languageForFile('schema.sql')).toBe('sql');
    expect(languageForFile('Main.java')).toBe('java');
    expect(languageForFile('app.properties')).toBe('properties');
    expect(languageForFile('README.md')).toBeNull();
  });
});

describe('preview limits', () => {
  it('warns about files over the 100 MB preview limit', () => {
    const warning = previewLimitWarning('pdf', PREVIEW_MAX_BYTES + 1);
    expect(warning?.title).toContain('слишком большой');
    expect(warning?.message).toContain('100 МБ');
    expect(warning?.limitBytes).toBe(PREVIEW_MAX_BYTES);
  });

  it('warns about text over the 512 KB highlight limit', () => {
    const warning = previewLimitWarning('code', PREVIEW_TEXT_MAX_BYTES + 1);
    expect(warning?.message).toContain('512 КБ');
    expect(warning?.limitBytes).toBe(PREVIEW_TEXT_MAX_BYTES);
  });

  it('allows files within limits and non-previewable types', () => {
    expect(previewLimitWarning('pdf', 1024)).toBeNull();
    expect(previewLimitWarning('code', 1024)).toBeNull();
    expect(previewLimitWarning('none', PREVIEW_MAX_BYTES * 2)).toBeNull();
  });

  it('treats the exact limit as allowed and non-positive sizes as unknown', () => {
    expect(previewLimitWarning('pdf', PREVIEW_MAX_BYTES)).toBeNull();
    expect(previewLimitWarning('image', PREVIEW_MAX_BYTES)).toBeNull();
    expect(previewLimitWarning('text', PREVIEW_TEXT_MAX_BYTES)).toBeNull();
    expect(previewLimitWarning('code', PREVIEW_TEXT_MAX_BYTES)).toBeNull();
    expect(previewLimitWarning('pdf', 0)).toBeNull();
    expect(previewLimitWarning('pdf', -1)).toBeNull();
    expect(previewLimitWarning('pdf', Number.NaN)).toBeNull();
  });
});

describe('preview extension parsing', () => {
  it('does not preview dotfiles, trailing dots or nested archives', () => {
    expect(selectViewer('.env')).toBe('none');
    expect(selectViewer('.hidden')).toBe('none');
    expect(selectViewer('report.')).toBe('none');
    expect(selectViewer('backup.tar.gz')).toBe('none');
    expect(selectViewer('archive.zip')).toBe('none');
    expect(canPreviewFile('file', '.env')).toBe(false);
  });

  it('ignores only the last extension and matches case-insensitively', () => {
    expect(selectViewer('NOTES.TXT')).toBe('text');
    expect(selectViewer('CONFIG.JSON')).toBe('code');
    expect(languageForFile('CONFIG.JSON')).toBe('json');
    expect(languageForFile('photo.jpeg.backup')).toBeNull();
  });
});
