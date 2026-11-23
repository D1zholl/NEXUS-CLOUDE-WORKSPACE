import { fireEvent, render, screen } from '@testing-library/react';
import { MemoryRouter, Route, Routes } from 'react-router-dom';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { PublicSharePage } from './PublicSharePage';

function jsonResponse(status: number, body: unknown): Response {
  const text = JSON.stringify(body);
  return {
    ok: status >= 200 && status < 300,
    status,
    json: async () => body,
    text: async () => text,
    blob: async () => new Blob(['payload']),
  } as unknown as Response;
}

function renderShare(token = 'share-token') {
  return render(
    <MemoryRouter initialEntries={[`/s/${token}`]}>
      <Routes>
        <Route path="/s/:token" element={<PublicSharePage />} />
      </Routes>
    </MemoryRouter>,
  );
}

beforeEach(() => {
  vi.unstubAllGlobals();
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe('PublicSharePage', () => {
  it('asks an auth-only link visitor to log in and opens the file after login', async () => {
    let shareRequests = 0;
    const fetchMock = vi.fn(async (input: RequestInfo | URL) => {
      const url = String(input);
      if (url.includes('/auth/login')) {
        return jsonResponse(200, {
          success: true,
          data: {
            user: {
              id: '11111111-1111-1111-1111-111111111111',
              username: 'viewer',
              displayName: 'Viewer',
              isSuperadmin: false,
              roles: [],
              permissions: ['cloud.file.download'],
            },
            csrfToken: 'csrf',
            session: { expiresAt: '2026-12-01T00:00:00Z' },
            modules: {},
          },
        });
      }
      shareRequests += 1;
      if (shareRequests === 1) {
        return jsonResponse(401, {
          success: false,
          error: { code: 'SHARE_AUTH_REQUIRED', message: 'Требуется вход.' },
        });
      }
      return jsonResponse(200, {
        success: true,
        data: {
          name: 'secret.txt',
          size: 7,
          contentType: 'text/plain',
          hasPassword: false,
          authOnly: true,
          downloadCount: 0,
          maxDownloads: null,
          expiresAt: null,
        },
      });
    });
    vi.stubGlobal('fetch', fetchMock);

    renderShare();

    expect(await screen.findByText('Только для авторизованных')).toBeTruthy();
    fireEvent.change(screen.getByLabelText('Логин'), { target: { value: 'viewer' } });
    fireEvent.change(screen.getByLabelText('Пароль'), {
      target: { value: 'SecretPass1!' },
    });
    fireEvent.click(screen.getByRole('button', { name: /Войти и скачать/ }));

    expect(await screen.findByText('secret.txt')).toBeTruthy();
    const loginCall = fetchMock.mock.calls.find(([url]) => String(url).includes('/auth/login'));
    expect(loginCall).toBeTruthy();
    expect(shareRequests).toBe(2);
  });

  it('reports a wrong password on a protected link without putting it in the URL', async () => {
    const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input);
      if (url.endsWith('/unlock')) {
        const body = String(init?.body ?? '');
        if (body.includes('"password":"wrong"')) {
          return jsonResponse(403, {
            success: false,
            error: { code: 'SHARE_PASSWORD_INVALID', message: 'Неверный пароль.' },
          });
        }
        return jsonResponse(200, {
          success: true,
          data: { ticket: 'ticket-value', expiresAt: '2026-12-01T00:00:00Z' },
        });
      }
      return jsonResponse(200, {
        success: true,
        data: {
          name: 'protected.zip',
          size: 10,
          contentType: 'application/zip',
          hasPassword: true,
          authOnly: false,
          downloadCount: 0,
          maxDownloads: 3,
          expiresAt: null,
        },
      });
    });
    vi.stubGlobal('fetch', fetchMock);

    renderShare();

    expect(await screen.findByText('protected.zip')).toBeTruthy();
    const downloadButton = screen.getByRole('button', { name: /Скачать/ }) as HTMLButtonElement;
    expect(downloadButton.disabled).toBe(true);

    fireEvent.change(screen.getByLabelText('Пароль'), { target: { value: 'wrong' } });
    fireEvent.click(downloadButton);

    expect(await screen.findByText('Неверный пароль.')).toBeTruthy();
    const urls = fetchMock.mock.calls.map(([url]) => String(url));
    expect(urls.some((url) => url.includes('/unlock'))).toBe(true);
    expect(urls.some((url) => url.includes('password='))).toBe(false);
  });

  it('downloads with a ticket after a successful unlock', async () => {
    const fetchMock = vi.fn(async (input: RequestInfo | URL) => {
      const url = String(input);
      if (url.endsWith('/unlock')) {
        return jsonResponse(200, {
          success: true,
          data: { ticket: 'ticket-value', expiresAt: '2026-12-01T00:00:00Z' },
        });
      }
      return jsonResponse(200, {
        success: true,
        data: {
          name: 'public.zip',
          size: 10,
          contentType: 'application/zip',
          hasPassword: false,
          authOnly: false,
          downloadCount: 0,
          maxDownloads: 1,
          expiresAt: null,
        },
      });
    });
    vi.stubGlobal('fetch', fetchMock);
    const click = vi.fn();
    const originalCreateElement = document.createElement.bind(document);
    const createElement = vi
      .spyOn(document, 'createElement')
      .mockImplementation((tagName: string) => {
        const element = originalCreateElement(tagName);
        if (tagName === 'a') {
          element.click = click;
        }
        return element;
      });

    renderShare();

    expect(await screen.findByText('public.zip')).toBeTruthy();
    fireEvent.click(screen.getByRole('button', { name: /Скачать файл/ }));

    expect(await screen.findByText('public.zip')).toBeTruthy();
    expect(fetchMock.mock.calls.some(([url]) => String(url).includes('/unlock'))).toBe(true);
    expect(click).toHaveBeenCalled();
    const link = createElement.mock.results.find(
      (result) => (result.value as HTMLElement).tagName === 'A',
    )?.value as HTMLAnchorElement | undefined;
    expect(link?.href).toContain('ticket=ticket-value');
    createElement.mockRestore();
  });
});
