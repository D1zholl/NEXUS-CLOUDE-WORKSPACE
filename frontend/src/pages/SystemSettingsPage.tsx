import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { useState } from 'react';
import type { ReactNode } from 'react';
import { RotateCcw, Save } from 'lucide-react';

import { Alert, Badge, Button, Field, LoadingInline, PageHead, Switch } from '../components/ui';
import {
  api,
  errorMessage,
  type SystemSettingSpec,
  type SystemSettingsResponse,
} from '../services/api';
import { toast } from '../stores/ui';

/**
 * Универсальная страница группы системных настроек.
 *
 * Значения, требующие перезапуска, показываются только для чтения: нельзя
 * создавать видимость изменения, которое не применится до рестарта сервера.
 */
export function SettingsGroupPage({
  eyebrow,
  title,
  description,
  keys,
  extra,
}: {
  eyebrow: string;
  title: string;
  description: string;
  keys: string[];
  extra?: ReactNode;
}) {
  const queryClient = useQueryClient();
  const settings = useQuery({
    queryKey: ['system-settings'],
    queryFn: api.getSystemSettings,
  });

  const save = useMutation({
    mutationFn: async ({ key, value }: { key: string; value: unknown }) => {
      await api.saveSystemSetting(key, value);
      return { key, value };
    },
    onSuccess: async () => {
      toast('Настройка сохранена.', 'success');
      await queryClient.invalidateQueries({ queryKey: ['system-settings'] });
    },
    onError: (error) => toast(errorMessage(error), 'error'),
  });

  if (settings.isPending) return <LoadingInline label="Загрузка настроек" />;
  if (settings.isError || !settings.data) {
    return (
      <>
        <PageHead eyebrow={eyebrow} title={title} description={description} />
        <Alert>{errorMessage(settings.error)}</Alert>
      </>
    );
  }

  const data: SystemSettingsResponse = settings.data;
  const specs = data.catalog.filter((spec) => keys.includes(spec.key));

  return (
    <>
      <PageHead eyebrow={eyebrow} title={title} description={description} />
      <div className="stack">
        {specs.map((spec) => (
          <SettingField
            key={spec.key}
            spec={spec}
            value={data.values[spec.key]}
            effective={data.effective[spec.key]}
            saving={save.isPending && save.variables?.key === spec.key}
            onSave={(value) => save.mutate({ key: spec.key, value })}
          />
        ))}
        {specs.length === 0 ? (
          <Alert tone="warn">
            В реестре сервера нет ключей этой группы. Проверьте версию backend.
          </Alert>
        ) : null}
      </div>
      {extra}
    </>
  );
}

function SettingField({
  spec,
  value,
  effective,
  saving,
  onSave,
}: {
  spec: SystemSettingSpec;
  /** Сохранённое значение из system_settings (может отсутствовать). */
  value: unknown;
  /** Эффективное значение, которое действует в данный момент. */
  effective: unknown;
  saving: boolean;
  onSave: (value: unknown) => void;
}) {
  const saved = value ?? defaultValue(spec);
  // Показываем действующее значение, а не минимум реестра и не «ожидающее»
  // значение restart-only ключа.
  const current = effective ?? saved;
  const [draft, setDraft] = useState<unknown>(current);
  const dirty = JSON.stringify(draft) !== JSON.stringify(current);
  const readOnly = spec.requiresRestart;
  const pendingRestart =
    readOnly && value !== undefined && JSON.stringify(value) !== JSON.stringify(effective);
  const hint = readOnly
    ? pendingRestart
      ? `Сохранённое значение ${formatValue(value)} вступит в силу после перезапуска службы; сейчас действует ${formatValue(effective)}.`
      : 'Только чтение: значение применяется при старте службы. Измените его через API и перезапустите сервер.'
    : `Применяется без перезапуска. Эффективное значение: ${formatValue(effective ?? current)}.`;

  return (
    <div className="card" style={{ padding: 18 }} data-testid={`system-setting-${spec.key}`}>
      <div className="row row--between" style={{ gap: 16, alignItems: 'flex-start' }}>
        <div style={{ minWidth: 0 }}>
          <div className="row" style={{ gap: 8, flexWrap: 'wrap' }}>
            <strong style={{ fontSize: 13.5 }}>{spec.label}</strong>
            <code className="tiny muted">{spec.key}</code>
            {spec.requiresRestart ? (
              <Badge tone="warn" plain>
                после перезапуска
              </Badge>
            ) : null}
            {pendingRestart ? (
              <Badge tone="danger" plain>
                ожидает перезапуска
              </Badge>
            ) : null}
          </div>
          {spec.min !== null || spec.max !== null ? (
            <div className="tiny muted" style={{ marginTop: 4 }}>
              Допустимый диапазон: {spec.min ?? '—'} … {spec.max ?? '—'}
            </div>
          ) : null}
        </div>
        {!readOnly ? (
          <Button
            size="sm"
            variant={dirty ? 'primary' : 'ghost'}
            icon={<Save aria-hidden="true" />}
            disabled={!dirty}
            loading={saving}
            onClick={() => onSave(draft)}
          >
            Сохранить
          </Button>
        ) : (
          <Badge tone="info" plain>
            только чтение
          </Badge>
        )}
      </div>
      <div style={{ marginTop: 12 }}>
        <Field hint={hint}>{renderControl(spec, current, draft, setDraft, readOnly)}</Field>
      </div>
      {dirty && !readOnly ? (
        <button
          type="button"
          className="link-button tiny"
          style={{ marginTop: 8 }}
          onClick={() => setDraft(current)}
        >
          <RotateCcw aria-hidden="true" size={12} /> Отменить изменение
        </button>
      ) : null}
    </div>
  );
}

/** Компактное представление значения настройки для подсказки. */
function formatValue(value: unknown): string {
  if (value === undefined || value === null) return '—';
  if (typeof value === 'string') return value;
  if (typeof value === 'number' || typeof value === 'boolean') return String(value);
  try {
    return JSON.stringify(value);
  } catch {
    return String(value);
  }
}

function renderControl(
  spec: SystemSettingSpec,
  current: unknown,
  draft: unknown,
  setDraft: (value: unknown) => void,
  readOnly: boolean,
): ReactNode {
  if (spec.kind === 'bool') {
    return (
      <Switch
        checked={Boolean(draft)}
        disabled={readOnly}
        label={draft ? 'Включено' : 'Выключено'}
        onChange={(next) => setDraft(next)}
      />
    );
  }
  if (spec.kind === 'int') {
    return (
      <input
        className="input"
        type="number"
        min={spec.min ?? undefined}
        max={spec.max ?? undefined}
        value={Number(draft ?? 0)}
        disabled={readOnly}
        onChange={(event) => setDraft(Number(event.target.value))}
      />
    );
  }
  if (spec.kind === 'enum' && spec.options) {
    return (
      <select
        className="input"
        value={String(draft ?? '')}
        disabled={readOnly}
        onChange={(event) => setDraft(event.target.value)}
      >
        {spec.options.map((option) => (
          <option key={option} value={option}>
            {option}
          </option>
        ))}
      </select>
    );
  }
  if (spec.kind === 'json') {
    return (
      <textarea
        className="input"
        rows={4}
        value={typeof draft === 'string' ? draft : JSON.stringify(draft ?? current, null, 2)}
        disabled={readOnly}
        onChange={(event) => {
          try {
            setDraft(JSON.parse(event.target.value));
          } catch {
            setDraft(event.target.value);
          }
        }}
      />
    );
  }
  return (
    <input
      className="input"
      value={String(draft ?? '')}
      disabled={readOnly}
      onChange={(event) => setDraft(event.target.value)}
    />
  );
}

function defaultValue(spec: SystemSettingSpec): unknown {
  if (spec.kind === 'bool') return false;
  if (spec.kind === 'int') return spec.min ?? 0;
  if (spec.kind === 'enum' && spec.options?.length) return spec.options[0];
  if (spec.kind === 'json') return spec.key === 'network.allowed_origins' ? [] : {};
  return '';
}