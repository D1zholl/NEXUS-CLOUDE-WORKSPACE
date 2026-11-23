import { useEffect, useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import {
  Maximize2,
  Minimize2,
  Palette,
  RotateCcw,
  Save,
  Server,
  Sparkles,
  Undo2,
} from 'lucide-react';

import { BackgroundCanvas } from '../components/Background';
import {
  Alert,
  Badge,
  Button,
  ConfirmDialog,
  LoadingInline,
  PageHead,
  RangeControl,
  SectionTitle,
  Segmented,
  Switch,
} from '../components/ui';
import {
  BACKGROUND_PRESETS,
  DEFAULT_BACKGROUND,
  PALETTE_SWATCHES,
  PRESET_ORDER,
  sanitizeBackground,
} from '../lib/background';
import { api, errorMessage } from '../services/api';
import { useBackground, isBackgroundDirty } from '../stores/background';
import { useCan } from '../stores/auth';
import { toast } from '../stores/ui';

const PERCENT = (value: number): string => `${Math.round(value * 100)}%`;
const DECIMAL = (value: number): string => value.toFixed(2).replace('.', ',');

export function BackgroundSettingsPage() {
  const queryClient = useQueryClient();
  const settings = useBackground((state) => state.settings);
  const loading = useBackground((state) => state.loading);
  const loaded = useBackground((state) => state.loaded);
  const saving = useBackground((state) => state.saving);
  const load = useBackground((state) => state.load);
  const update = useBackground((state) => state.update);
  const applyPreset = useBackground((state) => state.applyPreset);
  const discard = useBackground((state) => state.discard);
  const reset = useBackground((state) => state.reset);
  const save = useBackground((state) => state.save);
  const dirty = useBackground(isBackgroundDirty);

  const [expanded, setExpanded] = useState(false);
  const [confirmDefaultReset, setConfirmDefaultReset] = useState(false);
  const canManageDefaults = useCan('system.settings.manage');

  useEffect(() => {
    void load();
  }, [load]);

  const systemSettings = useQuery({
    queryKey: ['system-settings'],
    queryFn: api.getSystemSettings,
    enabled: canManageDefaults,
    retry: false,
  });

  const saveDefault = useMutation({
    mutationFn: (value: typeof settings) =>
      api.saveSystemSetting('appearance.background.defaults', value),
    onSuccess: async () => {
      toast('Системный фон по умолчанию обновлён.', 'success');
      await queryClient.invalidateQueries({ queryKey: ['system-settings'] });
    },
    onError: (error) => toast(errorMessage(error), 'error'),
  });

  const hasSystemDefault =
    systemSettings.data?.values?.['appearance.background.defaults'] !== undefined;

  return (
    <>
      <PageHead
        eyebrow="Аккаунт"
        title="3D-фон"
        description="Параметры 3D-фона хранятся в вашем профиле. Изменения применяются к предпросмотру сразу, но сохраняются только по кнопке."
        actions={
          dirty ? (
            <Badge tone="warn">Есть несохранённые изменения</Badge>
          ) : (
            <Badge tone="ok">Настройки сохранены</Badge>
          )
        }
      />

      {!loaded && loading ? <LoadingInline label="Загрузка параметров фона" /> : null}

      <div className="bg-settings">
        <div className={`bg-preview${expanded ? ' bg-preview--expanded' : ''}`}>
          <BackgroundCanvas settings={settings} preview />
          <div className="bg-preview__overlay">
            <div className="row row--between">
              <Badge tone="info" plain>
                <Sparkles aria-hidden="true" style={{ width: 12, height: 12 }} />
                Предпросмотр 3D
              </Badge>
              <Button
                size="sm"
                variant="ghost"
                onClick={() => setExpanded((value) => !value)}
                icon={
                  expanded ? <Minimize2 aria-hidden="true" /> : <Maximize2 aria-hidden="true" />
                }
              >
                {expanded ? 'Свернуть' : 'Развернуть'}
              </Button>
            </div>
            <div className="bg-preview__hint">
              {settings.enabled
                ? 'Наведите курсор на область предпросмотра: камера реагирует на движение. Настройки применяются в реальном времени.'
                : '3D-фон отключён — отображается статичная подложка. Включите переключатель в панели параметров.'}
            </div>
          </div>
        </div>

        <aside className="bg-controls">
          <div className="bg-controls__head">
            <Palette aria-hidden="true" style={{ width: 16, height: 16, color: 'var(--accent)' }} />
            <div className="grow">
              <div style={{ fontSize: 13, fontWeight: 600 }}>Параметры фона</div>
              <div className="tiny muted">
                {settings.preset === 'custom'
                  ? 'Пользовательская настройка'
                  : `Пресет: ${BACKGROUND_PRESETS[settings.preset as keyof typeof BACKGROUND_PRESETS]?.name ?? settings.preset}`}
              </div>
            </div>
            {dirty ? <span className="bg-unsaved">не сохранено</span> : null}
          </div>

          <div className="bg-controls__scroll">
            <div className="settings-section">
              <span className="settings-section__title">Пресеты</span>
              <div className="preset-row">
                {PRESET_ORDER.map((preset) => (
                  <button
                    key={preset}
                    type="button"
                    className={`preset${settings.preset === preset ? ' preset--active' : ''}`}
                    onClick={() => applyPreset(preset)}
                  >
                    <span
                      className="preset__dot"
                      style={{
                        background: PALETTE_SWATCHES.find(
                          (item) => item.key === BACKGROUND_PRESETS[preset].patch.palette,
                        )?.gradient,
                      }}
                    />
                    {BACKGROUND_PRESETS[preset].name}
                  </button>
                ))}
              </div>
            </div>

            <div className="settings-section">
              <span className="settings-section__title">Палитра</span>
              <div className="swatches">
                {PALETTE_SWATCHES.map((swatch) => (
                  <button
                    key={swatch.key}
                    type="button"
                    className="swatch"
                    style={{ background: swatch.gradient }}
                    aria-pressed={settings.palette === swatch.key}
                    aria-label={`Палитра ${swatch.name}`}
                    title={swatch.name}
                    onClick={() => update({ palette: swatch.key })}
                  />
                ))}
              </div>
            </div>

            <div className="settings-section">
              <span className="settings-section__title">Основное</span>
              <div className="settings-section__grid">
                <Switch
                  checked={settings.enabled}
                  onChange={(enabled) => update({ enabled })}
                  label="3D-фон включён"
                />
                <Switch
                  checked={settings.wireframe}
                  onChange={(wireframe) => update({ wireframe })}
                  label="Каркасный режим"
                />
                <RangeControl
                  label="Интенсивность"
                  min={0}
                  max={2}
                  step={0.05}
                  value={settings.intensity}
                  onChange={(intensity) => update({ intensity })}
                  format={DECIMAL}
                />
                <RangeControl
                  label="Свечение"
                  min={0}
                  max={2.5}
                  step={0.05}
                  value={settings.glow}
                  onChange={(glow) => update({ glow })}
                  format={DECIMAL}
                />
                <RangeControl
                  label="Прозрачность"
                  min={0.05}
                  max={1}
                  step={0.01}
                  value={settings.opacity}
                  onChange={(opacity) => update({ opacity })}
                  format={PERCENT}
                />
                <RangeControl
                  label="Перспектива"
                  min={0.5}
                  max={2}
                  step={0.05}
                  value={settings.perspective}
                  onChange={(perspective) => update({ perspective })}
                  format={DECIMAL}
                />
              </div>
            </div>

            <div className="settings-section">
              <span className="settings-section__title">Геометрия</span>
              <div className="settings-section__grid">
                <RangeControl
                  label="Количество кубов"
                  min={40}
                  max={700}
                  step={10}
                  value={settings.count}
                  onChange={(count) => update({ count })}
                />
                <RangeControl
                  label="Минимальный размер"
                  min={0.2}
                  max={2}
                  step={0.05}
                  value={settings.sizeMin}
                  onChange={(sizeMin) => update({ sizeMin })}
                  format={DECIMAL}
                />
                <RangeControl
                  label="Максимальный размер"
                  min={0.25}
                  max={3}
                  step={0.05}
                  value={settings.sizeMax}
                  onChange={(sizeMax) => update({ sizeMax })}
                  format={DECIMAL}
                />
                <RangeControl
                  label="Плотность"
                  min={0.2}
                  max={2.5}
                  step={0.05}
                  value={settings.spacing}
                  onChange={(spacing) => update({ spacing })}
                  format={DECIMAL}
                />
                <RangeControl
                  label="Глубина"
                  min={0.2}
                  max={2}
                  step={0.05}
                  value={settings.depth}
                  onChange={(depth) => update({ depth })}
                  format={DECIMAL}
                />
                <RangeControl
                  label="Разброс"
                  min={0.2}
                  max={2}
                  step={0.05}
                  value={settings.spread}
                  onChange={(spread) => update({ spread })}
                  format={DECIMAL}
                />
              </div>
            </div>

            <div className="settings-section">
              <span className="settings-section__title">Движение</span>
              <div className="settings-section__grid">
                <RangeControl
                  label="Скорость кубов"
                  min={0}
                  max={3}
                  step={0.05}
                  value={settings.speed}
                  onChange={(speed) => update({ speed })}
                  format={DECIMAL}
                />
                <RangeControl
                  label="Вращение"
                  min={0}
                  max={3}
                  step={0.05}
                  value={settings.rotation}
                  onChange={(rotation) => update({ rotation })}
                  format={DECIMAL}
                />
                <RangeControl
                  label="Амплитуда камеры"
                  min={0}
                  max={1}
                  step={0.01}
                  value={settings.cameraAmplitude}
                  onChange={(cameraAmplitude) => update({ cameraAmplitude })}
                  format={PERCENT}
                />
                <RangeControl
                  label="Параллакс"
                  min={0}
                  max={1}
                  step={0.01}
                  value={settings.parallax}
                  onChange={(parallax) => update({ parallax })}
                  format={PERCENT}
                />
                <RangeControl
                  label="Чувствительность мыши"
                  min={0}
                  max={1}
                  step={0.01}
                  value={settings.mouseSensitivity}
                  onChange={(mouseSensitivity) => update({ mouseSensitivity })}
                  format={PERCENT}
                />
                <RangeControl
                  label="Скорость звёзд"
                  min={0}
                  max={3}
                  step={0.05}
                  value={settings.backgroundSpeed}
                  onChange={(backgroundSpeed) => update({ backgroundSpeed })}
                  format={DECIMAL}
                />
              </div>
            </div>

            <div className="settings-section">
              <span className="settings-section__title">Звёзды и пыль</span>
              <div className="settings-section__grid">
                <RangeControl
                  label="Количество звёзд"
                  min={0}
                  max={2500}
                  step={50}
                  value={settings.stars}
                  onChange={(stars) => update({ stars })}
                />
                <RangeControl
                  label="Яркость звёзд"
                  min={0}
                  max={1}
                  step={0.01}
                  value={settings.starBrightness}
                  onChange={(starBrightness) => update({ starBrightness })}
                  format={PERCENT}
                />
                <RangeControl
                  label="Мерцание"
                  min={0}
                  max={1}
                  step={0.01}
                  value={settings.twinkle}
                  onChange={(twinkle) => update({ twinkle })}
                  format={PERCENT}
                />
                <RangeControl
                  label="Размер звёзд"
                  min={0.1}
                  max={3}
                  step={0.05}
                  value={settings.starSize}
                  onChange={(starSize) => update({ starSize })}
                  format={DECIMAL}
                />
                <RangeControl
                  label="Глубина звёзд"
                  min={0.2}
                  max={2}
                  step={0.05}
                  value={settings.starDepth}
                  onChange={(starDepth) => update({ starDepth })}
                  format={DECIMAL}
                />
              </div>
            </div>

            <div className="settings-section">
              <span className="settings-section__title">Постобработка</span>
              <div className="settings-section__grid">
                <RangeControl
                  label="Виньетка"
                  min={0}
                  max={1}
                  step={0.01}
                  value={settings.vignette}
                  onChange={(vignette) => update({ vignette })}
                  format={PERCENT}
                />
                <RangeControl
                  label="Зерно"
                  min={0}
                  max={1}
                  step={0.01}
                  value={settings.grain}
                  onChange={(grain) => update({ grain })}
                  format={PERCENT}
                />
                <RangeControl
                  label="Хроматическая аберрация"
                  min={0}
                  max={1}
                  step={0.01}
                  value={settings.aberration}
                  onChange={(aberration) => update({ aberration })}
                  format={PERCENT}
                />
              </div>
            </div>

            <div className="settings-section">
              <span className="settings-section__title">Производительность</span>
              <div className="settings-section__grid">
                <div>
                  <div className="field__label" style={{ marginBottom: 7 }}>
                    Качество рендера
                  </div>
                  <Segmented
                    value={settings.quality}
                    options={[
                      { value: 'auto', label: 'Авто' },
                      { value: 'high', label: 'Высокое' },
                      { value: 'eco', label: 'Эконом' },
                    ]}
                    onChange={(quality) => update({ quality })}
                    label="Качество рендера"
                  />
                </div>
                <div>
                  <div className="field__label" style={{ marginBottom: 7 }}>
                    Лимит FPS
                  </div>
                  <Segmented
                    value={String(settings.fpsLimit)}
                    options={[
                      { value: '0', label: 'Без лимита' },
                      { value: '30', label: '30' },
                      { value: '60', label: '60' },
                      { value: '120', label: '120' },
                    ]}
                    onChange={(fps) => update({ fpsLimit: Number(fps) })}
                    label="Лимит кадров"
                  />
                </div>
              </div>
            </div>

            {canManageDefaults ? (
              <div className="settings-section">
                <span className="settings-section__title">Системные значения</span>
                <div className="stack" style={{ gap: 8 }}>
                  <div className="tiny muted" style={{ lineHeight: 1.6 }}>
                    Значение по умолчанию применяется к пользователям без собственных настроек.
                    {hasSystemDefault
                      ? ' Сейчас задано системное значение.'
                      : ' Сейчас используются заводские параметры.'}
                  </div>
                  <Button
                    size="sm"
                    block
                    icon={<Server aria-hidden="true" />}
                    loading={saveDefault.isPending}
                    onClick={() => saveDefault.mutate(settings)}
                  >
                    Сохранить как системный default
                  </Button>
                  <Button
                    size="sm"
                    block
                    variant="ghost"
                    icon={<RotateCcw aria-hidden="true" />}
                    loading={saveDefault.isPending}
                    onClick={() => setConfirmDefaultReset(true)}
                  >
                    Сбросить системный default
                  </Button>
                </div>
              </div>
            ) : null}

            <div className="settings-section">
              <span className="settings-section__title">Сброс</span>
              <Button
                size="sm"
                block
                variant="ghost"
                icon={<Undo2 aria-hidden="true" />}
                onClick={reset}
              >
                Вернуть заводские параметры
              </Button>
              <div className="tiny muted" style={{ marginTop: 7, lineHeight: 1.5 }}>
                Изменения не сохранятся, пока вы не нажмёте «Сохранить». Кнопка «Отменить» вернёт
                последнее сохранённое состояние.
              </div>
            </div>
          </div>

          <div className="bg-controls__foot">
            <Button
              variant="primary"
              icon={<Save aria-hidden="true" />}
              loading={saving}
              disabled={!dirty}
              onClick={() => void save()}
            >
              Сохранить
            </Button>
            <Button variant="ghost" disabled={!dirty || saving} onClick={discard}>
              Отменить
            </Button>
          </div>
        </aside>
      </div>

      {systemSettings.isError && canManageDefaults ? (
        <Alert tone="warn">
          Не удалось загрузить системные значения: {errorMessage(systemSettings.error)}
        </Alert>
      ) : null}

      <SectionTitle title="Прозрачность настроек" meta="Хранение в PostgreSQL" />
      <div className="card">
        <div className="info-list">
          <div className="info-list__row">
            <span>Ключ пользовательской настройки</span>
            <span className="mono">appearance.background</span>
          </div>
          <div className="info-list__row">
            <span>Ключ системного значения</span>
            <span className="mono">appearance.background.defaults</span>
          </div>
          <div className="info-list__row">
            <span>Серверная валидация</span>
            <span>количество, размеры и лимиты ограничены</span>
          </div>
          <div className="info-list__row">
            <span>Хранение файлов</span>
            <span>настройки — в БД, ничего не пишется на диск</span>
          </div>
        </div>
      </div>

      <ConfirmDialog
        open={confirmDefaultReset}
        title="Сбросить системный default?"
        description="Системное значение вернётся к заводским параметрам. Пользователи без собственных настроек получат этот фон."
        confirmLabel="Сбросить"
        busy={saveDefault.isPending}
        onCancel={() => setConfirmDefaultReset(false)}
        onConfirm={() => {
          setConfirmDefaultReset(false);
          saveDefault.mutate(sanitizeBackground(DEFAULT_BACKGROUND));
        }}
      />
    </>
  );
}
