/** Просмотр изменений: unified diff и количество добавленных/удалённых строк. */
import { useMemo, useState } from 'react';
import { Clipboard, Download, Loader2 } from 'lucide-react';

import { computeDiff, unifiedDiff } from '../../../lib/lineDiff';
import type { DiffLine } from '../../../lib/lineDiff';
import { toast } from '../../../stores/ui';

interface SplitRow {
  left: DiffLine | null;
  right: DiffLine | null;
}

function splitRows(lines: DiffLine[]): SplitRow[] {
  const rows: SplitRow[] = [];
  let left: DiffLine[] = [];
  let right: DiffLine[] = [];
  const flush = () => {
    const height = Math.max(left.length, right.length);
    for (let index = 0; index < height; index += 1) {
      rows.push({ left: left[index] ?? null, right: right[index] ?? null });
    }
    left = [];
    right = [];
  };
  for (const line of lines) {
    if (line.type === 'context') {
      flush();
      rows.push({ left: line, right: line });
    } else if (line.type === 'del') {
      left.push(line);
    } else {
      right.push(line);
    }
  }
  flush();
  return rows;
}

interface DiffViewerProps {
  path: string;
  before: string;
  after: string;
  loading?: boolean;
  truncated?: boolean;
}

export function DiffViewer({ path, before, after, loading, truncated }: DiffViewerProps) {
  const [mode, setMode] = useState<'unified' | 'split'>('unified');
  const diff = useMemo(() => computeDiff(before, after), [before, after]);
  const rows = useMemo(() => splitRows(diff.lines), [diff.lines]);

  if (loading) {
    return (
      <div className="diff-viewer diff-viewer--loading">
        <Loader2 className="spin" aria-hidden="true" />
        <span>Загрузка версии файла…</span>
      </div>
    );
  }

  const copy = async () => {
    try {
      await navigator.clipboard.writeText(unifiedDiff(path, before, after));
      toast('Diff скопирован в буфер обмена.', 'success');
    } catch {
      toast('Буфер обмена недоступен.', 'warning');
    }
  };

  const download = () => {
    const blob = new Blob([unifiedDiff(path, before, after)], {
      type: 'text/x-diff;charset=utf-8',
    });
    const url = URL.createObjectURL(blob);
    const link = document.createElement('a');
    link.href = url;
    link.download = `${path.replace(/[\\/]/g, '_')}.diff`;
    link.click();
    window.setTimeout(() => URL.revokeObjectURL(url), 2000);
  };

  return (
    <div className="diff-viewer">
      <div className="diff-viewer__bar">
        <span className="diff-viewer__path">{path}</span>
        <span className="diff-viewer__stat diff-viewer__stat--add">+{diff.additions}</span>
        <span className="diff-viewer__stat diff-viewer__stat--del">-{diff.deletions}</span>
        {truncated || diff.truncated ? (
          <span className="badge badge--warn">Файл большой: сравнение упрощено</span>
        ) : null}
        <div className="diff-viewer__actions">
          <button
            type="button"
            className="btn btn--sm btn--ghost"
            onClick={() => setMode(mode === 'unified' ? 'split' : 'unified')}
          >
            {mode === 'unified' ? 'Две колонки' : 'Unified'}
          </button>
          <button type="button" className="btn btn--sm btn--ghost" onClick={() => void copy()}>
            <Clipboard aria-hidden="true" /> Копировать
          </button>
          <button type="button" className="btn btn--sm btn--ghost" onClick={download}>
            <Download aria-hidden="true" /> Скачать
          </button>
        </div>
      </div>
      {diff.additions === 0 && diff.deletions === 0 ? (
        <div className="diff-viewer__empty">Файл не изменён.</div>
      ) : mode === 'unified' ? (
        <div className="diff-viewer__body">
          {diff.lines.map((line, index) => (
            <div
              key={index}
              className={`diff-line diff-line--${line.type}`}
            >
              <span className="diff-line__num">{line.oldLine ?? ''}</span>
              <span className="diff-line__num">{line.newLine ?? ''}</span>
              <span className="diff-line__mark">
                {line.type === 'add' ? '+' : line.type === 'del' ? '-' : ' '}
              </span>
              <span className="diff-line__text">{line.text || ' '}</span>
            </div>
          ))}
        </div>
      ) : (
        <div className="diff-viewer__split">
          <div className="diff-viewer__side">
            <div className="diff-viewer__side-title">Было</div>
            {rows.map((row, index) => (
              <div
                key={index}
                className={`diff-line diff-line--${
                  row.left ? (row.left.type === 'del' ? 'del' : 'context') : 'empty'
                }`}
              >
                <span className="diff-line__num">{row.left?.oldLine ?? ''}</span>
                <span className="diff-line__text">{row.left ? row.left.text || ' ' : ''}</span>
              </div>
            ))}
          </div>
          <div className="diff-viewer__side">
            <div className="diff-viewer__side-title">Стало</div>
            {rows.map((row, index) => (
              <div
                key={index}
                className={`diff-line diff-line--${
                  row.right ? (row.right.type === 'add' ? 'add' : 'context') : 'empty'
                }`}
              >
                <span className="diff-line__num">{row.right?.newLine ?? ''}</span>
                <span className="diff-line__text">{row.right ? row.right.text || ' ' : ''}</span>
              </div>
            ))}
          </div>
        </div>
      )}
    </div>
  );
}
