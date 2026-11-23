import { useEffect, useState } from 'react';
import { useQuery } from '@tanstack/react-query';
import { ArrowLeft, ArrowRight, Check, Loader2 } from 'lucide-react';

import { Alert, Badge, Button, Field, Modal } from '../components/ui';
import { api, errorMessage, type CreateBuildInput, type LoaderType } from '../services/api';
import { toast } from '../stores/ui';

/**
 * Пошаговый мастер создания сборки (PLAN §7.3): 5 шагов с валидацией.
 *
 * Шаг 1 — основные данные, 2 — версия Minecraft, 3 — загрузчик,
 * 4 — требования к Java (автоподстановка по версии игры), 5 — итог и создание.
 * Список версий загрузчика фильтруется на сервере (`/metadata/loaders`).
 */

const STEPS = ['Основные данные', 'Minecraft', 'Загрузчик', 'Java', 'Готово'] as const;

const LOADERS: Array<{ value: LoaderType; label: string }> = [
  { value: 'vanilla', label: 'Vanilla' },
  { value: 'fabric', label: 'Fabric' },
  { value: 'forge', label: 'Forge' },
  { value: 'neoforge', label: 'NeoForge' },
];

const CATEGORIES = [
  { value: 'tech', label: 'Технические' },
  { value: 'magic', label: 'Магия' },
  { value: 'adventure', label: 'Приключения' },
  { value: 'optimization', label: 'Оптимизация' },
  { value: 'other', label: 'Другое' },
];

/** Обязательная версия Java по версии Minecraft (правило MyLauncher). */
export function javaForMinecraft(version: string): number {
  const parts = version.split('.');
  const major = Number.parseInt(parts[0] ?? '', 10);
  const minor = Number.parseInt((parts[1] ?? '').replace(/[^0-9].*$/, ''), 10);
  const patch = Number.parseInt((parts[2] ?? '').replace(/[^0-9].*$/, ''), 10) || 0;
  if (Number.isNaN(major)) return 21;
  if (major === 1) {
    if (Number.isNaN(minor)) return 21;
    if (minor <= 16) return 8;
    if (minor === 17) return 16;
    if (minor <= 19) return 17;
    if (minor === 20) return patch < 5 ? 17 : 21;
    return 21;
  }
  return major >= 25 ? 25 : 21;
}

const DRAFT_KEY = 'nexus.build.wizard.draft';

interface Draft {
  name: string;
  slug: string;
  description: string;
  version: string;
  category: string;
  visibility: 'public' | 'private';
  minecraftVersion: string;
  loaderType: LoaderType;
  loaderVersion: string;
  javaMajor: number;
  javaMin: number;
  javaNotes: string;
}

const EMPTY: Draft = {
  name: '',
  slug: '',
  description: '',
  version: '1.0.0',
  category: 'other',
  visibility: 'private',
  minecraftVersion: '',
  loaderType: 'vanilla',
  loaderVersion: '',
  javaMajor: 21,
  javaMin: 21,
  javaNotes: '',
};

function slugify(name: string): string {
  const map: Record<string, string> = {
    а: 'a',
    б: 'b',
    в: 'v',
    г: 'g',
    д: 'd',
    е: 'e',
    ё: 'e',
    ж: 'zh',
    з: 'z',
    и: 'i',
    й: 'y',
    к: 'k',
    л: 'l',
    м: 'm',
    н: 'n',
    о: 'o',
    п: 'p',
    р: 'r',
    с: 's',
    т: 't',
    у: 'u',
    ф: 'f',
    х: 'h',
    ц: 'ts',
    ч: 'ch',
    ш: 'sh',
    щ: 'sch',
    ъ: '',
    ы: 'y',
    ь: '',
    э: 'e',
    ю: 'yu',
    я: 'ya',
  };
  return name
    .toLowerCase()
    .split('')
    .map((ch) => map[ch] ?? ch)
    .join('')
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-+|-+$/g, '')
    .slice(0, 64);
}

export function BuildWizard({
  open,
  onClose,
  onCreated,
}: {
  open: boolean;
  onClose: () => void;
  onCreated: (id: string) => void;
}) {
  const [step, setStep] = useState(0);
  const [draft, setDraft] = useState<Draft>(() => loadDraft() ?? EMPTY);
  const [slugTouched, setSlugTouched] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const versions = useQuery({
    queryKey: ['mc-versions'],
    queryFn: () => api.minecraftVersions(),
    enabled: open,
    staleTime: 10 * 60 * 1000,
  });

  const loaders = useQuery({
    queryKey: ['mc-loaders', draft.loaderType, draft.minecraftVersion],
    queryFn: () => api.loaderVersions(draft.loaderType, draft.minecraftVersion),
    enabled: open && draft.loaderType !== 'vanilla' && draft.minecraftVersion.trim().length > 0,
    staleTime: 5 * 60 * 1000,
    retry: false,
  });

  const patch = (next: Partial<Draft>): void => {
    setDraft((current) => {
      const merged = { ...current, ...next };
      if (next.name !== undefined && !slugTouched) {
        merged.slug = slugify(next.name);
      }
      if (next.minecraftVersion !== undefined && next.minecraftVersion) {
        const java = javaForMinecraft(next.minecraftVersion);
        if (java !== current.javaMajor) {
          merged.javaMajor = java;
          merged.javaMin = java;
        }
      }
      saveDraft(merged);
      return merged;
    });
  };

  // Версия загрузчика всегда берётся из списка совместимых с игрой.
  useEffect(() => {
    if (draft.loaderType === 'vanilla' || !loaders.isSuccess) return;
    const versions = loaders.data?.versions ?? [];
    setDraft((current) => {
      if (
        current.loaderType !== draft.loaderType ||
        current.minecraftVersion !== draft.minecraftVersion
      ) {
        return current;
      }
      const nextVersion = versions.includes(current.loaderVersion)
        ? current.loaderVersion
        : (versions[0] ?? '');
      if (nextVersion === current.loaderVersion) return current;
      const merged = { ...current, loaderVersion: nextVersion };
      saveDraft(merged);
      return merged;
    });
  }, [draft.loaderType, draft.minecraftVersion, loaders.isSuccess, loaders.data]);

  const stepValid = (index: number): boolean => {
    if (index === 0) {
      return (
        draft.name.trim().length >= 2 &&
        draft.slug.trim().length >= 3 &&
        /^[a-z0-9](?:[a-z0-9-]*[a-z0-9])$/.test(draft.slug.trim())
      );
    }
    if (index === 1) return draft.minecraftVersion.trim().length > 0;
    if (index === 2) {
      if (draft.loaderType === 'vanilla') return true;
      return (loaders.data?.versions ?? []).includes(draft.loaderVersion);
    }
    if (index === 3) return draft.javaMin <= draft.javaMajor && draft.javaMajor >= 8;
    return true;
  };

  const next = (): void => {
    if (!stepValid(step)) {
      setError('Заполните обязательные поля этого шага.');
      return;
    }
    setError(null);
    setStep((current) => Math.min(current + 1, STEPS.length - 1));
  };

  const back = (): void => {
    setError(null);
    setStep((current) => Math.max(current - 1, 0));
  };

  const submit = async (): Promise<void> => {
    setBusy(true);
    setError(null);
    try {
      const input: CreateBuildInput = {
        name: draft.name.trim(),
        slug: draft.slug.trim(),
        description: draft.description.trim(),
        version: draft.version.trim() || '1.0.0',
        category: draft.category,
        visibility: draft.visibility,
        minecraftVersion: draft.minecraftVersion.trim(),
        loaderType: draft.loaderType,
        loaderVersion: draft.loaderType === 'vanilla' ? '' : draft.loaderVersion.trim(),
        javaMajor: draft.javaMajor,
        javaMin: draft.javaMin,
        javaNotes: draft.javaNotes.trim(),
      };
      const build = await api.createBuild(input);
      clearDraft();
      toast('Сборка создана. Добавьте файлы и опубликуйте релиз.', 'success');
      onCreated(build.id);
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setBusy(false);
    }
  };

  const close = (): void => {
    setError(null);
    onClose();
  };

  return (
    <Modal
      open={open}
      onClose={busy ? () => undefined : close}
      wide
      title="Новая сборка"
      description="Пять шагов: данные, версия игры, загрузчик, Java и создание."
      footer={
        <>
          <Button onClick={close} disabled={busy}>
            Отмена
          </Button>
          <div className="grow" />
          {step > 0 ? (
            <Button icon={<ArrowLeft aria-hidden="true" />} onClick={back} disabled={busy}>
              Назад
            </Button>
          ) : null}
          {step < STEPS.length - 1 ? (
            <Button variant="primary" onClick={next} icon={<ArrowRight aria-hidden="true" />}>
              Далее
            </Button>
          ) : (
            <Button variant="primary" onClick={() => void submit()} loading={busy}>
              Создать сборку
            </Button>
          )}
        </>
      }
    >
      <div
        className="wizard-steps"
        style={{ display: 'flex', gap: 6, marginBottom: 18, flexWrap: 'wrap' }}
      >
        {STEPS.map((label, index) => (
          <div
            key={label}
            className={`wizard-step${index === step ? ' wizard-step--active' : ''}${
              index < step ? ' wizard-step--done' : ''
            }`}
            style={{
              display: 'flex',
              alignItems: 'center',
              gap: 6,
              fontSize: 11.5,
              padding: '4px 10px',
              borderRadius: 999,
              border: '1px solid var(--border-soft)',
              color: index === step ? 'var(--accent)' : 'var(--text-secondary)',
            }}
          >
            {index < step ? <Check aria-hidden="true" size={12} /> : <span>{index + 1}</span>}
            {label}
          </div>
        ))}
      </div>

      {error ? <Alert tone="danger">{error}</Alert> : null}

      <div className="wizard-body" key={step}>
        {step === 0 ? (
        <div className="form-grid">
          <Field label="Название сборки" htmlFor="wizard-name" hint="2–96 символов">
            <input
              id="wizard-name"
              className="input"
              value={draft.name}
              maxLength={96}
              onChange={(event) => patch({ name: event.target.value })}
            />
          </Field>
          <Field
            label="Slug"
            htmlFor="wizard-slug"
            hint="3–64 символа: латиница, цифры и дефис; без дефиса на концах"
          >
            <input
              id="wizard-slug"
              className="input mono"
              value={draft.slug}
              maxLength={64}
              onChange={(event) => {
                setSlugTouched(true);
                patch({ slug: event.target.value.toLowerCase() });
              }}
            />
          </Field>
          <Field label="Описание" htmlFor="wizard-description" full>
            <textarea
              id="wizard-description"
              className="textarea"
              value={draft.description}
              maxLength={2000}
              onChange={(event) => patch({ description: event.target.value })}
            />
          </Field>
          <Field label="Версия сборки" htmlFor="wizard-version">
            <input
              id="wizard-version"
              className="input mono"
              value={draft.version}
              maxLength={32}
              onChange={(event) => patch({ version: event.target.value })}
            />
          </Field>
          <Field label="Категория" htmlFor="wizard-category">
            <select
              id="wizard-category"
              className="select"
              value={draft.category}
              onChange={(event) => patch({ category: event.target.value })}
            >
              {CATEGORIES.map((item) => (
                <option key={item.value} value={item.value}>
                  {item.label}
                </option>
              ))}
            </select>
          </Field>
          <Field
            label="Видимость"
            htmlFor="wizard-visibility"
            hint="Приватная сборка не появится в MyLauncher: клиент без токена видит только публичные сборки."
          >
            <select
              id="wizard-visibility"
              className="select"
              value={draft.visibility}
              onChange={(event) =>
                patch({ visibility: event.target.value as 'public' | 'private' })
              }
            >
              <option value="private">Приватная</option>
              <option value="public">Публичная</option>
            </select>
          </Field>
          <Field label="Иконка" hint="Загрузится после создания сборки">
            <div className="tiny muted">PNG, JPEG или WEBP до 2 МБ.</div>
          </Field>
        </div>
      ) : null}

      {step === 1 ? (
        <Field label="Версия Minecraft" hint="Только официальные релизы Mojang">
          {versions.isPending ? (
            <div className="row" style={{ gap: 8 }}>
              <Loader2 aria-hidden="true" size={14} /> Загрузка версий…
            </div>
          ) : versions.isError ? (
            <Alert>{errorMessage(versions.error)}</Alert>
          ) : (
            <select
              className="select"
              value={draft.minecraftVersion}
              onChange={(event) => patch({ minecraftVersion: event.target.value })}
            >
              <option value="">Выберите версию</option>
              {(versions.data?.versions ?? []).map((version) => (
                <option key={version.id} value={version.id}>
                  {version.id}
                </option>
              ))}
            </select>
          )}
        </Field>
      ) : null}

      {step === 2 ? (
        <div className="form-grid">
          <Field label="Загрузчик" htmlFor="wizard-loader">
            <select
              id="wizard-loader"
              className="select"
              value={draft.loaderType}
              onChange={(event) =>
                patch({ loaderType: event.target.value as LoaderType, loaderVersion: '' })
              }
            >
              {LOADERS.map((item) => (
                <option key={item.value} value={item.value}>
                  {item.label}
                </option>
              ))}
            </select>
          </Field>
          {draft.loaderType !== 'vanilla' ? (
            <Field
              label="Версия загрузчика"
              hint={
                loaders.isPending
                  ? 'Ищем совместимые версии…'
                  : loaders.isError
                    ? 'Список совместимых версий недоступен. Создание сборки временно невозможно.'
                    : (loaders.data?.versions?.length ?? 0) === 0
                      ? 'Для этой версии игры нет совместимых версий загрузчика.'
                      : 'Показаны только версии, совместимые с выбранной версией Minecraft.'
              }
            >
              {loaders.isPending ? (
                <div className="row" style={{ gap: 8 }}>
                  <Loader2 aria-hidden="true" size={14} /> Загрузка версий…
                </div>
              ) : loaders.isError ? (
                <Alert tone="warn">{errorMessage(loaders.error)}</Alert>
              ) : (loaders.data?.versions?.length ?? 0) === 0 ? (
                <Alert tone="warn">
                  Для этой версии игры совместимых версий загрузчика не найдено.
                </Alert>
              ) : (
                <select
                  className="select"
                  value={draft.loaderVersion}
                  onChange={(event) => patch({ loaderVersion: event.target.value })}
                >
                  {(loaders.data?.versions ?? []).map((version) => (
                    <option key={version} value={version}>
                      {version}
                    </option>
                  ))}
                </select>
              )}
            </Field>
          ) : (
            <Field label="Загрузчик">
              <Badge tone="info" plain>
                Vanilla — дополнительная настройка не требуется
              </Badge>
            </Field>
          )}
        </div>
      ) : null}

      {step === 3 ? (
        <div className="form-grid">
          <Field label="Основная версия Java" htmlFor="wizard-java-major" hint="8–30">
            <input
              id="wizard-java-major"
              className="input"
              type="number"
              min={8}
              max={30}
              value={draft.javaMajor}
              onChange={(event) => {
                const value = Number(event.target.value);
                patch({ javaMajor: value, javaMin: Math.min(draft.javaMin, value) });
              }}
            />
          </Field>
          <Field label="Минимальная версия Java" htmlFor="wizard-java-min" hint="Не выше основной">
            <input
              id="wizard-java-min"
              className="input"
              type="number"
              min={8}
              max={draft.javaMajor}
              value={draft.javaMin}
              onChange={(event) => patch({ javaMin: Number(event.target.value) })}
            />
          </Field>
          <Field label="Примечания" htmlFor="wizard-java-notes" full>
            <textarea
              id="wizard-java-notes"
              className="textarea"
              value={draft.javaNotes}
              maxLength={2000}
              onChange={(event) => patch({ javaNotes: event.target.value })}
            />
          </Field>
          <Alert tone="info">
            Значение подставлено автоматически по версии Minecraft. Проверьте его перед публикацией.
          </Alert>
        </div>
      ) : null}

      {step === 4 ? (
        <div className="info-list">
          <div className="info-list__row">
            <span>Название</span>
            <span>{draft.name}</span>
          </div>
          <div className="info-list__row">
            <span>Slug</span>
            <span className="mono">{draft.slug}</span>
          </div>
          <div className="info-list__row">
            <span>Minecraft</span>
            <span className="mono">{draft.minecraftVersion}</span>
          </div>
          <div className="info-list__row">
            <span>Загрузчик</span>
            <span>
              {draft.loaderType}
              {draft.loaderType !== 'vanilla' ? ` ${draft.loaderVersion}` : ''}
            </span>
          </div>
          <div className="info-list__row">
            <span>Java</span>
            <span>
              {draft.javaMajor} (мин. {draft.javaMin})
            </span>
          </div>
          <div className="info-list__row">
            <span>Видимость</span>
            <span>{draft.visibility === 'public' ? 'Публичная' : 'Приватная'}</span>
          </div>
          <Alert tone="info">
            После создания откроется страница сборки: добавьте файлы и запустите проверку перед
            публикацией.
          </Alert>
          {draft.visibility === 'private' ? (
            <Alert tone="warn">
              Сборка приватная: после публикации она будет видна только в панели. Stock MyLauncher
              не передаёт токен и не покажет её в каталоге; видимость можно изменить на странице
              сборки.
            </Alert>
          ) : (
            <Alert tone="info">
              Сборка публичная: после публикации релиза она появится в каталоге MyLauncher.
            </Alert>
          )}
        </div>
      ) : null}
      </div>
    </Modal>
  );
}

function saveDraft(draft: Draft): void {
  try {
    window.sessionStorage.setItem(DRAFT_KEY, JSON.stringify(draft));
  } catch {
    // Хранилище может быть недоступно в приватном режиме.
  }
}

function loadDraft(): Draft | null {
  try {
    const raw = window.sessionStorage.getItem(DRAFT_KEY);
    if (!raw) return null;
    const parsed = JSON.parse(raw) as Partial<Draft>;
    return { ...EMPTY, ...parsed };
  } catch {
    return null;
  }
}

function clearDraft(): void {
  try {
    window.sessionStorage.removeItem(DRAFT_KEY);
  } catch {
    // ignore
  }
}
