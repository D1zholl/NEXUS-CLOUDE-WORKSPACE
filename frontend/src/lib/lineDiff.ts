/** Построчный diff для просмотра изменений AI (без внешних зависимостей). */

export type DiffLineType = 'context' | 'add' | 'del';

export interface DiffLine {
  type: DiffLineType;
  text: string;
  oldLine: number | null;
  newLine: number | null;
}

export interface DiffResult {
  lines: DiffLine[];
  additions: number;
  deletions: number;
  /** Слишком большой файл: показано упрощённое сравнение. */
  truncated: boolean;
}

const MAX_DIFF_LINES = 1200;

/** LCS-дифф. Для больших файлов — упрощённое сравнение без выравнивания. */
export function computeDiff(before: string, after: string): DiffResult {
  const oldLines = before.length === 0 ? [] : before.split('\n');
  const newLines = after.length === 0 ? [] : after.split('\n');
  if (oldLines.length > MAX_DIFF_LINES || newLines.length > MAX_DIFF_LINES) {
    const lines: DiffLine[] = [];
    oldLines.forEach((text, index) =>
      lines.push({ type: 'del', text, oldLine: index + 1, newLine: null }),
    );
    newLines.forEach((text, index) =>
      lines.push({ type: 'add', text, oldLine: null, newLine: index + 1 }),
    );
    return {
      lines,
      additions: newLines.length,
      deletions: oldLines.length,
      truncated: true,
    };
  }

  const rows = oldLines.length;
  const columns = newLines.length;
  const table = new Uint16Array((rows + 1) * (columns + 1));
  for (let row = rows - 1; row >= 0; row -= 1) {
    for (let column = columns - 1; column >= 0; column -= 1) {
      table[row * (columns + 1) + column] =
        oldLines[row] === newLines[column]
          ? table[(row + 1) * (columns + 1) + column + 1] + 1
          : Math.max(
              table[(row + 1) * (columns + 1) + column],
              table[row * (columns + 1) + column + 1],
            );
    }
  }

  const lines: DiffLine[] = [];
  let row = 0;
  let column = 0;
  let additions = 0;
  let deletions = 0;
  while (row < rows || column < columns) {
    if (row < rows && column < columns && oldLines[row] === newLines[column]) {
      lines.push({
        type: 'context',
        text: oldLines[row],
        oldLine: row + 1,
        newLine: column + 1,
      });
      row += 1;
      column += 1;
      continue;
    }
    const down =
      row < rows ? table[(row + 1) * (columns + 1) + column] : -1;
    const right =
      column < columns ? table[row * (columns + 1) + column + 1] : -1;
    if (row < rows && down >= right) {
      lines.push({ type: 'del', text: oldLines[row], oldLine: row + 1, newLine: null });
      deletions += 1;
      row += 1;
    } else {
      lines.push({ type: 'add', text: newLines[column], oldLine: null, newLine: column + 1 });
      additions += 1;
      column += 1;
    }
  }
  return { lines, additions, deletions, truncated: false };
}

/** Unified diff с 3 строками контекста; пригоден для копирования/скачивания. */
export function unifiedDiff(
  path: string,
  before: string,
  after: string,
  context = 3,
): string {
  const diff = computeDiff(before, after);
  if (diff.additions === 0 && diff.deletions === 0) return '';
  const keep = new Set<number>();
  diff.lines.forEach((line, index) => {
    if (line.type === 'context') return;
    for (
      let cursor = Math.max(0, index - context);
      cursor <= Math.min(diff.lines.length - 1, index + context);
      cursor += 1
    ) {
      keep.add(cursor);
    }
  });
  const output: string[] = [`--- a/${path}`, `+++ b/${path}`];
  let index = 0;
  while (index < diff.lines.length) {
    if (!keep.has(index)) {
      index += 1;
      continue;
    }
    const start = index;
    while (index < diff.lines.length && keep.has(index)) index += 1;
    const block = diff.lines.slice(start, index);
    const oldStart = block.find((line) => line.oldLine !== null)?.oldLine ?? 0;
    const newStart = block.find((line) => line.newLine !== null)?.newLine ?? 0;
    const oldCount = block.filter((line) => line.type !== 'add').length;
    const newCount = block.filter((line) => line.type !== 'del').length;
    output.push(`@@ -${oldStart},${oldCount} +${newStart},${newCount} @@`);
    for (const line of block) {
      output.push(
        `${line.type === 'add' ? '+' : line.type === 'del' ? '-' : ' '}${line.text}`,
      );
    }
  }
  return `${output.join('\n')}\n`;
}
