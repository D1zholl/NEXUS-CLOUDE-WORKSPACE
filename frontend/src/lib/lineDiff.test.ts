import { describe, expect, it } from 'vitest';

import { computeDiff, unifiedDiff } from './lineDiff';

describe('computeDiff', () => {
  it('returns only context for equal text', () => {
    const diff = computeDiff('a\nb\nc', 'a\nb\nc');
    expect(diff.additions).toBe(0);
    expect(diff.deletions).toBe(0);
    expect(diff.lines.every((line) => line.type === 'context')).toBe(true);
    expect(diff.lines).toHaveLength(3);
  });

  it('detects added and removed lines', () => {
    const diff = computeDiff('a\nb\nc', 'a\nB\nc\nd');
    expect(diff.deletions).toBe(1);
    expect(diff.additions).toBe(2);
    const added = diff.lines.filter((line) => line.type === 'add').map((line) => line.text);
    const removed = diff.lines.filter((line) => line.type === 'del').map((line) => line.text);
    expect(added).toEqual(['B', 'd']);
    expect(removed).toEqual(['b']);
    const firstAdd = diff.lines.find((line) => line.type === 'add');
    expect(firstAdd?.newLine).toBe(2);
    expect(firstAdd?.oldLine).toBeNull();
  });

  it('handles empty files', () => {
    const diff = computeDiff('', 'one\ntwo');
    expect(diff.additions).toBe(2);
    expect(diff.deletions).toBe(0);
    expect(computeDiff('one', '').deletions).toBe(1);
  });

  it('falls back for very large inputs', () => {
    const before = Array.from({ length: 1500 }, (_, index) => `line ${index}`).join('\n');
    const after = `${before}\nnew`;
    const diff = computeDiff(before, after);
    expect(diff.truncated).toBe(true);
  });
});

describe('unifiedDiff', () => {
  it('produces a unified diff with headers and context', () => {
    const before = 'a\nb\nc\nd\ne\nf\ng\nh';
    const after = 'a\nb\nc\nd\nE\nf\ng\nh';
    const text = unifiedDiff('src/a.ts', before, after);
    expect(text).toContain('--- a/src/a.ts');
    expect(text).toContain('+++ b/src/a.ts');
    expect(text).toMatch(/@@ -\d+,\d+ \+\d+,\d+ @@/);
    expect(text).toContain('-e');
    expect(text).toContain('+E');
    expect(text).toContain(' d');
  });

  it('returns empty string when nothing changed', () => {
    expect(unifiedDiff('a.txt', 'same', 'same')).toBe('');
  });
});
