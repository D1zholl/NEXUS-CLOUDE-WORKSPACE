import fs from 'node:fs';

import {
  expect,
  test as base,
  type BrowserContext,
  type Page,
  type TestInfo,
} from '@playwright/test';

export interface ConsoleIssue {
  kind: 'console.error' | 'pageerror' | 'csp';
  text: string;
  url?: string;
  line?: number;
  column?: number;
}

/**
 * 401/423/429 от auth-эндпоинтов — ожидаемый контракт (анонимный bootstrap,
 * неверный пароль, блокировка). Chromium печатает для них «Failed to load
 * resource» в консоль независимо от обработки ошибки в приложении.
 * Любая другая ошибка консоли или CSP остаётся падением теста.
 */
function isExpectedHttpError(issue: ConsoleIssue): boolean {
  if (issue.kind !== 'console.error') return false;
  if (!issue.text.includes('Failed to load resource')) return false;
  const url = issue.url ?? '';
  if (/\/api\/v1\/auth\/(me|login)$/.test(url)) {
    return /status of (401|423|429)/.test(issue.text);
  }
  // Штатный конфликт имени при auto-complete: UI предлагает «Заменить/Копия».
  if (/\/api\/v1\/cloud\/uploads\/[^/]+\/complete$/.test(url)) {
    return /status of 409/.test(issue.text);
  }
  // Идемпотентное переиспользование папки при загрузке/возобновлении папки.
  if (/\/api\/v1\/cloud\/entries$/.test(url) && /status of 409/.test(issue.text)) {
    return true;
  }
  // Сценарий возобновления намеренно обрывает канал на чанке: браузер пишет
  // сетевую ошибку, приложение обязано повторить запрос с серверного offset.
  if (
    /\/api\/v1\/cloud\/uploads\/[^/]+$/.test(url) &&
    /net::ERR_(CONNECTION_RESET|FAILED|ABORTED)/.test(issue.text)
  ) {
    return true;
  }
  // Agent Workspace: конфликт версий, конфликт откатa и отсутствие
  // baseSha256 — ожидаемые бизнес-ответы, которые UI показывает пользователю.
  if (
    /\/api\/v1\/ai\/workspaces\/[^/]+\/(files\/content|files\/upload|snapshots\/[^/]+\/restore|tasks\/[^/]+\/restore)$/.test(
      url,
    )
  ) {
    return /status of (409|412|428)/.test(issue.text);
  }
  return false;
}

interface CspViolation {
  blockedURI?: string;
  violatedDirective?: string;
  effectiveDirective?: string;
  sourceFile?: string;
  lineNumber?: number;
  columnNumber?: number;
}

/**
 * Собирает ошибки консоли, необработанные исключения и CSP-нарушения.
 * Каждый E2E-тест обязан завершиться без них (I1/H2).
 */
export class ConsoleGuard {
  private readonly issues: ConsoleIssue[] = [];
  private readonly contexts = new Set<BrowserContext>();
  private readonly pages = new Set<Page>();

  async attachContext(context: BrowserContext): Promise<void> {
    if (this.contexts.has(context)) return;
    this.contexts.add(context);
    await context.addInitScript(() => {
      const scope = window as unknown as { __nexusE2E?: { csp: CspViolation[] } };
      scope.__nexusE2E = { csp: [] };
      document.addEventListener(
        'securitypolicyviolation',
        (event) => {
          scope.__nexusE2E?.csp.push({
            blockedURI: event.blockedURI,
            violatedDirective: event.violatedDirective,
            effectiveDirective: event.effectiveDirective,
            sourceFile: event.sourceFile,
            lineNumber: event.lineNumber,
            columnNumber: event.columnNumber,
          });
        },
        true,
      );
    });
    for (const page of context.pages()) this.attachPage(page);
    context.on('page', (page) => this.attachPage(page));
  }

  attachPage(page: Page): void {
    if (this.pages.has(page)) return;
    this.pages.add(page);
    page.on('console', (message) => {
      if (message.type() !== 'error') return;
      const location = message.location();
      this.issues.push({
        kind: 'console.error',
        text: message.text(),
        url: location.url,
        line: location.lineNumber,
        column: location.columnNumber,
      });
    });
    page.on('pageerror', (error) => {
      this.issues.push({
        kind: 'pageerror',
        text: `${error.name}: ${error.message}`,
      });
    });
  }

  private async collectCsp(page: Page): Promise<void> {
    if (page.isClosed()) return;
    try {
      const violations = await page.evaluate(
        () =>
          ((window as unknown as { __nexusE2E?: { csp: CspViolation[] } }).__nexusE2E?.csp ??
            []) as CspViolation[],
      );
      for (const violation of violations) {
        this.issues.push({
          kind: 'csp',
          text:
            `CSP ${violation.effectiveDirective ?? violation.violatedDirective ?? '?'} ` +
            `blocked ${violation.blockedURI ?? '?'}`,
          url: violation.sourceFile,
          line: violation.lineNumber,
          column: violation.columnNumber,
        });
      }
      // Не дублируем нарушения при повторном вызове/навигации.
      await page.evaluate(() => {
        const scope = window as unknown as { __nexusE2E?: { csp: CspViolation[] } };
        if (scope.__nexusE2E) scope.__nexusE2E.csp = [];
      });
    } catch {
      // Страница уже закрыта или недоступна — консольные события уже собраны.
    }
  }

  async assertClean(testInfo: TestInfo): Promise<void> {
    for (const page of this.pages) await this.collectCsp(page);
    const unexpected = this.issues.filter((issue) => !isExpectedHttpError(issue));
    if (unexpected.length === 0) return;

    const payload = {
      test: testInfo.titlePath.join(' › '),
      issues: unexpected,
      ignored: this.issues.filter(isExpectedHttpError),
    };
    fs.writeFileSync(
      testInfo.outputPath('console-csp.json'),
      JSON.stringify(payload, null, 2),
      'utf8',
    );
    await testInfo.attach('console-csp.json', {
      path: testInfo.outputPath('console-csp.json'),
      contentType: 'application/json',
    });

    expect(
      unexpected,
      `Ошибки консоли/CSP в "${testInfo.title}":\n${unexpected
        .map((issue) => `- [${issue.kind}] ${issue.text}`)
        .join('\n')}`,
    ).toEqual([]);
  }
}

export interface E2EFixtures {
  consoleGuard: ConsoleGuard;
}

export const test = base.extend<E2EFixtures>({
  consoleGuard: [
    async ({ context }, use, testInfo) => {
      const guard = new ConsoleGuard();
      await guard.attachContext(context);
      testInfo.annotations.push({ type: 'console-guard', description: 'console + CSP' });
      await use(guard);
      await guard.assertClean(testInfo);
    },
    { auto: true },
  ],
});

export { expect };
