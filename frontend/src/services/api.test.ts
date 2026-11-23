import { describe, expect, it, vi, beforeEach, afterEach } from 'vitest';

import { ApiRequestError, api, errorMessage, newIdempotencyKey, setCsrfToken } from './api';

/**
 * Тесты клиента API: обработка кодов `MODULE_DISABLED`, `QUOTA_EXCEEDED`,
 * `RATE_LIMITED` и сетевых ошибок.
 */

function mockFetch(body: unknown, init: { status?: number; ok?: boolean } = {}) {
  const status = init.status ?? 200;
  const ok = init.ok ?? (status >= 200 && status < 300);
  return vi.fn(
    async () =>
      ({
        ok,
        status,
        headers: new Headers({ 'x-request-id': 'req-1' }),
        text: async () => JSON.stringify(body),
      }) as unknown as Response,
  );
}

beforeEach(() => {
  setCsrfToken('csrf');
});

afterEach(() => {
  vi.unstubAllGlobals();
  setCsrfToken(null);
});

describe('api client', () => {
  it('surfaces MODULE_DISABLED with server message', async () => {
    vi.stubGlobal(
      'fetch',
      mockFetch(
        {
          success: false,
          error: {
            code: 'MODULE_DISABLED',
            message: 'Модуль отключён администратором.',
            details: { module: 'cloud' },
          },
          request_id: 'req-1',
        },
        { status: 403 },
      ),
    );

    await expect(api.listCloud({})).rejects.toMatchObject({
      code: 'MODULE_DISABLED',
      status: 403,
    });
    await api.listCloud({}).catch((error: unknown) => {
      expect(errorMessage(error)).toContain('отключён администратором');
    });
  });

  it('surfaces QUOTA_EXCEEDED with details', async () => {
    vi.stubGlobal(
      'fetch',
      mockFetch(
        {
          success: false,
          error: {
            code: 'QUOTA_EXCEEDED',
            message: 'Превышена персональная квота хранения.',
            details: { limitBytes: 1000, usedBytes: 900, requestedBytes: 500 },
          },
        },
        { status: 413 },
      ),
    );

    await api.createCloudFolder({ name: 'x', parentId: null }).catch((error: unknown) => {
      expect(error).toBeInstanceOf(ApiRequestError);
      const failure = error as ApiRequestError;
      expect(failure.code).toBe('QUOTA_EXCEEDED');
      expect((failure.details as { limitBytes: number }).limitBytes).toBe(1000);
    });
  });

  it('surfaces RATE_LIMITED with retry_after', async () => {
    vi.stubGlobal(
      'fetch',
      mockFetch(
        {
          success: false,
          error: {
            code: 'RATE_LIMITED',
            message: 'Слишком много попыток. Повторите позже.',
            details: { retry_after_seconds: 12 },
          },
        },
        { status: 429 },
      ),
    );

    await api.overview().catch((error: unknown) => {
      const failure = error as ApiRequestError;
      expect(failure.status).toBe(429);
      expect((failure.details as { retry_after_seconds: number }).retry_after_seconds).toBe(12);
    });
  });

  it('reports network failure in user-friendly terms', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn(async () => {
        throw new TypeError('fetch failed');
      }),
    );
    await expect(api.health()).rejects.toMatchObject({ code: 'NETWORK_ERROR' });
  });

  it('adds the CSRF header for modifying requests', async () => {
    const fetchMock = mockFetch({ success: true, data: { ok: true } });
    vi.stubGlobal('fetch', fetchMock);
    await api.logout();
    const [, init] = fetchMock.mock.calls[0] as unknown as [string, RequestInit];
    const headers = init.headers as Record<string, string>;
    expect(headers['X-CSRF-Token']).toBe('csrf');
  });

  it('sends Idempotency-Key for dangerous operations', async () => {
    const fetchMock = mockFetch({ success: true, data: { ok: true } });
    vi.stubGlobal('fetch', fetchMock);

    await api.createCloudShare({ entryId: 'entry-1' }, 'share-key');
    const [, init] = fetchMock.mock.calls[0] as unknown as [string, RequestInit];
    const headers = init.headers as Record<string, string>;
    expect(headers['Idempotency-Key']).toBe('share-key');
  });

  it('creates a distinct key for each operation', () => {
    const first = newIdempotencyKey();
    const second = newIdempotencyKey();
    expect(first).toMatch(/^[0-9a-f-]{36}$|^idem-/);
    expect(second).not.toBe(first);
  });
});
