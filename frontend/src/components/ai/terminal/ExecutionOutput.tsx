/**
 * Вывод разрешённых программ.
 *
 * Долгоживущего терминала нет: `program.run` синхронный, поэтому панель
 * показывает реальные stdout/stderr/exitCode завершённых запусков, а не
 * интерактивную оболочку.
 */
import { useMemo, useState } from 'react';
import { AlertTriangle, Play, Terminal } from 'lucide-react';

import { Badge } from '../../ui';
import type { AiExecution } from '../../../services/ai';

interface ExecutionOutputProps {
  executions: AiExecution[];
}

function text(value: unknown): string {
  if (typeof value === 'string') return value;
  if (value === null || value === undefined) return '';
  try {
    return JSON.stringify(value, null, 2);
  } catch {
    return String(value);
  }
}

export function ExecutionOutput({ executions }: ExecutionOutputProps) {
  const runs = useMemo(
    () => executions.filter((execution) => execution.toolId === 'program.run'),
    [executions],
  );
  const [selected, setSelected] = useState<string | null>(null);
  const active = runs.find((run) => run.id === selected) ?? runs[runs.length - 1] ?? null;

  if (runs.length === 0) {
    return (
      <div className="exec-output exec-output--empty">
        <Terminal aria-hidden="true" />
        <p className="muted">
          Программы ещё не запускались. Агент запускает только зарегистрированные
          администратором программы через allowlist Runtime.
        </p>
      </div>
    );
  }

  const stdout = text(active?.result?.stdout);
  const stderr = text(active?.result?.stderr);

  return (
    <div className="exec-output">
      <div className="exec-output__bar">
        <Play aria-hidden="true" />
        {runs.length > 1 ? (
          <select
            value={active?.id ?? ''}
            onChange={(event) => setSelected(event.target.value)}
            aria-label="Выбор запуска"
          >
            {runs.map((run) => (
              <option key={run.id} value={run.id}>
                {String(run.arguments?.programId ?? 'program')} · {run.status}
              </option>
            ))}
          </select>
        ) : (
          <span>{String(active?.arguments?.programId ?? 'program')}</span>
        )}
        <Badge tone={active?.status === 'completed' ? 'ok' : 'danger'}>
          {active?.status ?? '—'}
        </Badge>
        {active?.exitCode !== null && active?.exitCode !== undefined ? (
          <span className="muted">exit {active.exitCode}</span>
        ) : null}
        <span className="muted">{active?.durationMs ?? 0} мс</span>
        <span className="exec-output__note">
          синхронный запуск · долгоживущий терминал не поддерживается
        </span>
      </div>
      {active?.errorMessage ? (
        <div className="exec-output__error">
          <AlertTriangle aria-hidden="true" /> {active.errorMessage}
        </div>
      ) : null}
      <div className="exec-output__grid">
        <div className="exec-output__panel">
          <span className="exec-output__label">stdout</span>
          <pre>{stdout || '(пусто)'}</pre>
        </div>
        <div className="exec-output__panel exec-output__panel--err">
          <span className="exec-output__label">stderr</span>
          <pre>{stderr || '(пусто)'}</pre>
        </div>
      </div>
    </div>
  );
}
