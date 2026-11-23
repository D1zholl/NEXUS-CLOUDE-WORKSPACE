import { describe, expect, it } from 'vitest';

import { javaForMinecraft } from '../components/BuildWizard';

/** Логика мастера сборки: подстановка Java по версии игры и валидация slug. */
describe('build wizard helpers', () => {
  it('suggests java 8 for legacy versions', () => {
    expect(javaForMinecraft('1.12.2')).toBe(8);
  });

  it('suggests java 17 for 1.18–1.20.4', () => {
    expect(javaForMinecraft('1.18.2')).toBe(17);
    expect(javaForMinecraft('1.19.4')).toBe(17);
    expect(javaForMinecraft('1.20.1')).toBe(17);
  });

  it('suggests java 21 for 1.20.5 and newer', () => {
    expect(javaForMinecraft('1.20.6')).toBe(21);
    expect(javaForMinecraft('1.21')).toBe(21);
  });

  it('matches MyLauncher for 1.17 and 25+', () => {
    expect(javaForMinecraft('1.17.1')).toBe(16);
    expect(javaForMinecraft('26.3')).toBe(25);
  });

  it('handles malformed versions conservatively', () => {
    expect(javaForMinecraft('')).toBe(21);
    expect(javaForMinecraft('snapshot')).toBe(21);
  });
});
