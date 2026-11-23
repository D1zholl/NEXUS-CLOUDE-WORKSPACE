import { beforeEach, describe, expect, it, vi } from 'vitest';

vi.mock('../services/api', () => ({
  api: {
    getMySettings: vi.fn(),
    getPublicBackground: vi.fn(),
    saveMySetting: vi.fn(),
  },
  errorMessage: (error: unknown) => (error instanceof Error ? error.message : String(error)),
}));

const apiModule = await import('../services/api');
const getMySettings = apiModule.api.getMySettings as ReturnType<typeof vi.fn>;
const getPublicBackground = apiModule.api.getPublicBackground as ReturnType<typeof vi.fn>;

const { resetBackgroundStore, useBackground } = await import('./background');

beforeEach(() => {
  resetBackgroundStore();
  vi.clearAllMocks();
});

describe('background store', () => {
  it('applies the system default returned for a user without own settings', async () => {
    getMySettings.mockResolvedValue({
      'appearance.background': {
        count: 100,
        speed: 0.55,
        palette: 'ice',
        preset: 'custom',
        enabled: true,
      },
    });

    await useBackground.getState().load(true);

    const state = useBackground.getState();
    expect(state.loaded).toBe(true);
    expect(state.settings.count).toBe(100);
    expect(state.settings.speed).toBe(0.55);
    expect(state.settings.palette).toBe('ice');
    expect(state.settings.preset).toBe('custom');
  });

  it('does not keep factory values after a failed first request', async () => {
    getMySettings.mockRejectedValueOnce(new Error('network down'));
    await useBackground.getState().load(true);

    // Ошибка не считается успешной загрузкой: повтор должен применить default.
    expect(useBackground.getState().loaded).toBe(false);
    expect(useBackground.getState().error).toContain('network down');

    getMySettings.mockResolvedValueOnce({
      'appearance.background': { count: 123, palette: 'magma', preset: 'magma' },
    });
    await useBackground.getState().load(true);

    expect(useBackground.getState().loaded).toBe(true);
    expect(useBackground.getState().settings.count).toBe(123);
    expect(useBackground.getState().settings.palette).toBe('magma');
  });

  it('uses the public system background before login', async () => {
    getPublicBackground.mockResolvedValueOnce({
      count: 321,
      palette: 'ice',
      preset: 'custom',
      enabled: true,
    });

    await useBackground.getState().loadPublic();

    const state = useBackground.getState();
    expect(state.settings.count).toBe(321);
    expect(state.settings.palette).toBe('ice');
    // После входа личные настройки всё ещё должны загрузиться.
    expect(state.loaded).toBe(false);

    getMySettings.mockResolvedValueOnce({
      'appearance.background': { count: 5, palette: 'magma', preset: 'magma' },
    });
    await useBackground.getState().load(true);

    expect(useBackground.getState().loaded).toBe(true);
    expect(useBackground.getState().settings.count).toBe(5);
  });

  it('ignores a stale public response after personal settings are loaded', async () => {
    let resolvePublic: (value: unknown) => void = () => undefined;
    getPublicBackground.mockReturnValueOnce(
      new Promise((resolve) => {
        resolvePublic = resolve;
      }),
    );
    getMySettings.mockResolvedValueOnce({
      'appearance.background': { count: 11, palette: 'magma', preset: 'magma' },
    });

    const publicRequest = useBackground.getState().loadPublic();
    await useBackground.getState().load(true);
    resolvePublic({ count: 999, palette: 'ice', enabled: true });
    await publicRequest;

    // Ответ для формы входа пришёл позже личных настроек и не должен их затереть.
    expect(useBackground.getState().settings.count).toBe(11);
    expect(useBackground.getState().settings.palette).toBe('magma');
  });
});
