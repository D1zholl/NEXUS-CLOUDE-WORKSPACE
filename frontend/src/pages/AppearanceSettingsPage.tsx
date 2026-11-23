import { useEffect } from 'react';
import { useNavigate } from 'react-router-dom';
import { ArrowRight, Check, Palette, Save, Sparkles, Undo2 } from 'lucide-react';

import { Badge, Button, PageHead, SectionTitle } from '../components/ui';
import { ACCENTS } from '../lib/appearance';
import { useAppearance, isAppearanceDirty } from '../stores/appearance';
import { useBackground } from '../stores/background';

/**
 * K4: страница §9.1 п. 4 «Внешний вид».
 *
 * Отделена от §9.1 п. 5 «Настройки 3D-фона» (`/system/background`). Здесь —
 * тема и акцент; сам фон настраивается на
 * соседней странице.
 */
export function AppearanceSettingsPage() {
  const navigate = useNavigate();
  const settings = useAppearance((state) => state.settings);
  const saved = useAppearance((state) => state.saved);
  const loaded = useAppearance((state) => state.loaded);
  const loading = useAppearance((state) => state.loading);
  const saving = useAppearance((state) => state.saving);
  const error = useAppearance((state) => state.error);
  const load = useAppearance((state) => state.load);
  const update = useAppearance((state) => state.update);
  const discard = useAppearance((state) => state.discard);
  const save = useAppearance((state) => state.save);
  const dirty = useAppearance(isAppearanceDirty);

  const backgroundEnabled = useBackground((state) => state.settings.enabled);

  useEffect(() => {
    void load();
  }, [load]);

  const currentAccent = ACCENTS.find((item) => item.key === settings.accent) ?? ACCENTS[0];

  return (
    <>
      <PageHead
        eyebrow="Аккаунт"
        title="Внешний вид"
        description="Тема и акцент интерфейса хранятся в вашем профиле. Настройки 3D-фона вынесены в отдельный раздел."
        actions={
          dirty ? (
            <Badge tone="warn">Есть несохранённые изменения</Badge>
          ) : (
            <Badge tone="ok">Настройки сохранены</Badge>
          )
        }
      />

      {error ? <Badge tone="warn">{error}</Badge> : null}

      <div className="grid-2">
        <section className="card">
          <SectionTitle title="Тема интерфейса" meta="NEXUS Dark" />
          <p className="muted" style={{ marginTop: 0, lineHeight: 1.6 }}>
            Основная тема — тёмная неоновая. Светлая тема в 1.0 не поставляется: тёмная палитра
            обеспечивает контраст на всех страницах и совпадает с оформлением 3D-фона.
          </p>
          <div className="row" style={{ gap: 8, alignItems: 'center' }}>
            <Badge tone="info" plain>
              <Palette aria-hidden="true" style={{ width: 12, height: 12 }} />
              Активный акцент: {currentAccent.name}
            </Badge>
            {dirty ? <span className="tiny muted">предпросмотр применён</span> : null}
          </div>

          <SectionTitle title="Акцент" meta="применяется сразу" />
          <div className="swatches" role="group" aria-label="Акцентный цвет">
            {ACCENTS.map((accent) => {
              const active = settings.accent === accent.key;
              return (
                <button
                  key={accent.key}
                  type="button"
                  className="swatch"
                  style={{ background: accent.gradient }}
                  aria-pressed={active}
                  aria-label={accent.name}
                  title={`${accent.name} — ${accent.description}`}
                  onClick={() => update({ accent: accent.key })}
                >
                  {active ? (
                    <Check aria-hidden="true" style={{ width: 14, height: 14, color: '#041018' }} />
                  ) : null}
                </button>
              );
            })}
          </div>
          <div className="info-list" style={{ marginTop: 12 }}>
            <div className="info-list__row">
              <span>Сохранённый акцент</span>
              <span className="mono">{saved.accent}</span>
            </div>
            <div className="info-list__row">
              <span>Хранение</span>
              <span className="mono">appearance.interface</span>
            </div>
          </div>
        </section>

        <section className="card">
          <SectionTitle title="Движение" meta="анимация фона" />
          <div className="stack" style={{ gap: 10 }}>
            <span data-testid="motion-state">
              <Badge tone="ok">Анимация включена</Badge>
            </span>
            <p className="muted" style={{ margin: 0, lineHeight: 1.6 }}>
              Анимация интерфейса и 3D-фона не зависит от системного ограничения движения.
            </p>
            <div className="tiny muted" style={{ lineHeight: 1.6 }}>
              Частоту кадров и качество рендера можно настроить в разделе 3D-фона.
            </div>
          </div>
        </section>
      </div>

      <section className="card">
        <SectionTitle title="3D-фон" meta="отдельный раздел §9.1 п. 5" />
        <div className="row row--between" style={{ gap: 12, flexWrap: 'wrap' }}>
          <div>
            <div style={{ fontWeight: 600, marginBottom: 4 }}>
              3D-фон {backgroundEnabled ? 'включён' : 'отключён'}
            </div>
            <div className="tiny muted" style={{ lineHeight: 1.6 }}>
              Пресеты, палитра, геометрия, частицы, постобработка и системные значения по умолчанию
              настраиваются отдельно.
            </div>
          </div>
          <Button
            variant="primary"
            icon={<Sparkles aria-hidden="true" />}
            onClick={() => navigate('/system/background')}
          >
            Настроить 3D-фон
            <ArrowRight aria-hidden="true" style={{ width: 14, height: 14 }} />
          </Button>
        </div>
      </section>

      <div className="row" style={{ gap: 8 }}>
        <Button
          variant="primary"
          icon={<Save aria-hidden="true" />}
          loading={saving}
          disabled={!dirty || !loaded || loading}
          onClick={() => void save()}
        >
          Сохранить
        </Button>
        <Button
          variant="ghost"
          icon={<Undo2 aria-hidden="true" />}
          disabled={!dirty || saving}
          onClick={discard}
        >
          Отменить
        </Button>
      </div>
    </>
  );
}
