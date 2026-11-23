import { Link } from 'react-router-dom';
import {
  ArrowLeft,
  Braces,
  Code2,
  FileCode2,
  Lightbulb,
  MonitorSmartphone,
  Server,
  Sparkles,
  Terminal,
  Users,
} from 'lucide-react';

import { LogoMark } from '../components/Brand';

interface TeamMember {
  name: string;
  role: string;
  detail: string;
  icon: typeof Code2;
}

interface Language {
  name: string;
  usage: string;
  icon: typeof Code2;
}

const TEAM: TeamMember[] = [
  {
    name: 'DeepSeek V4.1 Flash 1017',
    role: 'Написание кода всей системы',
    detail: 'Реализация бэкенда, интерфейса, интеграций и инфраструктурных сценариев NEXUS.',
    icon: Code2,
  },
  {
    name: 'Qwen4 и ChatGPT',
    role: 'Планирование и развитие проекта',
    detail:
      'Планирование, написание подробных промтов по требованиям, рекомендации по развитию проекта.',
    icon: Lightbulb,
  },
  {
    name: 'Maks',
    role: 'Управление командой разработки',
    detail: 'Чаткурьер: постановка задач, обратная связь и координация работы команды.',
    icon: Users,
  },
];

const LANGUAGES: Language[] = [
  { name: 'Rust', usage: 'бэкенд, лаунчер, CLI', icon: Server },
  { name: 'TypeScript / TSX', usage: 'React-интерфейс', icon: MonitorSmartphone },
  { name: 'JavaScript', usage: 'Node.js-скрипты и e2e-тесты', icon: Braces },
  { name: 'CSS', usage: 'темы и оформление', icon: Sparkles },
  { name: 'HTML', usage: 'точка входа приложения', icon: FileCode2 },
  { name: 'SQL', usage: 'миграции PostgreSQL', icon: Server },
  { name: 'PowerShell', usage: 'Windows-скрипты и деплой', icon: Terminal },
  { name: 'Batch', usage: 'запуск и остановка сервера', icon: Terminal },
];

/**
 * Публичная страница о разработчиках.
 *
 * Доступна без авторизации: на неё ведёт кликабельный блок с логотипом
 * на странице входа. Данных из системы не запрашивает.
 */
export function DevelopersPage() {
  return (
    <div className="developers">
      <div className="developers__shell">
        <header className="developers__top">
          <Link
            to="/login"
            className="developers__brand"
            title="NEXUS WORKSPACE"
            aria-label="NEXUS WORKSPACE — на страницу входа"
          >
            <LogoMark size={22} />
            <div>
              <div className="brand__text">NEXUS WORKSPACE</div>
              <span className="brand__sub">ЦИФРОВОЕ ПРОСТРАНСТВО</span>
            </div>
          </Link>
          <Link to="/login" className="btn btn--sm btn--ghost">
            <ArrowLeft aria-hidden="true" />
            Ко входу
          </Link>
        </header>

        <section className="developers__hero">
          <div className="eyebrow">О проекте</div>
          <h1 className="h1">
            Разработчики <span className="developers__accent">NEXUS WORKSPACE</span>
          </h1>
          <p>
            Самостоятельно размещаемое рабочее пространство для релизов, файлов и совместной работы
            команды. Ниже — команда, инструменты и языки, на которых построена платформа.
          </p>
        </section>

        <section className="developers__section" aria-labelledby="developers-team">
          <div className="developers__section-head">
            <h2 id="developers-team" className="h3">
              Команда
            </h2>
            <span className="developers__count">{TEAM.length}</span>
          </div>
          <div className="developers__grid">
            {TEAM.map((member) => {
              const Icon = member.icon;
              return (
                <article key={member.name} className="developers__member glass">
                  <div className="developers__member-icon">
                    <Icon aria-hidden="true" />
                  </div>
                  <div className="developers__member-body">
                    <h3>{member.name}</h3>
                    <div className="developers__role">{member.role}</div>
                    <p>{member.detail}</p>
                  </div>
                </article>
              );
            })}
          </div>
        </section>

        <section className="developers__section" aria-labelledby="developers-stack">
          <div className="developers__section-head">
            <h2 id="developers-stack" className="h3">
              Технологии
            </h2>
          </div>
          <div className="developers__meta-grid">
            <div className="developers__meta-card glass">
              <div className="developers__meta-icon">
                <Terminal aria-hidden="true" />
              </div>
              <div>
                <div className="developers__meta-label">Среда разработки</div>
                <div className="developers__meta-value">opencode</div>
                <p>Агентная среда, в которой создаётся код и ведутся проверки проекта.</p>
              </div>
            </div>

            <div className="developers__meta-card glass">
              <div className="developers__meta-icon">
                <Braces aria-hidden="true" />
              </div>
              <div className="developers__meta-content">
                <div className="developers__meta-label">Языки программирования</div>
                <div className="developers__languages">
                  {LANGUAGES.map((language) => {
                    const Icon = language.icon;
                    return (
                      <div key={language.name} className="developers__language">
                        <Icon aria-hidden="true" />
                        <div>
                          <span className="developers__language-name">{language.name}</span>
                          <span className="developers__language-usage">{language.usage}</span>
                        </div>
                      </div>
                    );
                  })}
                </div>
                <p className="developers__formats">
                  Форматы конфигурации: TOML, JSON, Caddyfile, .env
                </p>
              </div>
            </div>
          </div>
        </section>

        <footer className="developers__foot">
          <span>NEXUS WORKSPACE · локальная инфраструктура</span>
          <Link to="/login">Вернуться ко входу</Link>
        </footer>
      </div>
    </div>
  );
}

export default DevelopersPage;
