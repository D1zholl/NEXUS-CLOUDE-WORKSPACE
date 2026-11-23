import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { SettingsGroupPage } from './SystemSettingsPage';
import type { SystemSettingSpec, SystemSettingsResponse } from '../services/api';

vi.mock('../services/api', () => ({
  api: {
    getSystemSettings: vi.fn(),
    saveSystemSetting: vi.fn(),
  },
  errorMessage: (error: unknown) => (error instanceof Error ? error.message : 'ошибка'),
}));

const apiModule = await import('../services/api');
const mockApi = apiModule.api as unknown as {
  getSystemSettings: ReturnType<typeof vi.fn>;
  saveSystemSetting: ReturnType<typeof vi.fn>;
};

const INT_SPEC: SystemSettingSpec = {
  key: 'limits.max_upload_mb',
  kind: 'int',
  min: 1,
  max: 102400,
  requiresRestart: false,
  permission: 'system.settings.manage',
  label: 'Максимальный размер файла, МБ',
  options: null,
};

const ORIGINS_SPEC: SystemSettingSpec = {
  key: 'network.allowed_origins',
  kind: 'json',
  min: null,
  max: null,
  requiresRestart: true,
  permission: 'system.settings.manage',
  label: 'Разрешённые источники (CORS)',
  options: null,
};

const OTHER_SPEC: SystemSettingSpec = {
  ...INT_SPEC,
  key: 'jobs.max_concurrent',
  label: 'Лимит параллельных задач',
  min: 1,
  max: 8,
};

function response(overrides: Partial<SystemSettingsResponse> = {}): SystemSettingsResponse {
  return {
    catalog: [INT_SPEC, ORIGINS_SPEC, OTHER_SPEC],
    values: {
      'limits.max_upload_mb': 64,
      'network.allowed_origins': [],
      'jobs.max_concurrent': 2,
    },
    effective: {
      'limits.max_upload_mb': 64,
      'network.allowed_origins': [],
      'jobs.max_concurrent': 2,
    },
    meta: {},
    ...overrides,
  };
}

function renderGroup(keys: string[]) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <SettingsGroupPage
        eyebrow="Система"
        title="Лимиты"
        description="Проверка формы"
        keys={keys}
      />
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  vi.clearAllMocks();
  mockApi.getSystemSettings.mockResolvedValue(response());
  mockApi.saveSystemSetting.mockResolvedValue(512);
});

describe('SettingsGroupPage', () => {
  it('renders only the keys of the requested group', async () => {
    renderGroup(['limits.max_upload_mb']);

    expect(await screen.findByText('Максимальный размер файла, МБ')).toBeTruthy();
    expect(screen.queryByText('Разрешённые источники (CORS)')).toBeNull();
    expect(screen.queryByText('Лимит параллельных задач')).toBeNull();
    expect(mockApi.getSystemSettings).toHaveBeenCalledTimes(1);
  });

  it('saves an int field as a number through the typed control', async () => {
    renderGroup(['limits.max_upload_mb']);

    const input = (await screen.findByRole('spinbutton')) as HTMLInputElement;
    fireEvent.change(input, { target: { value: '512' } });
    expect(input.value).toBe('512');

    fireEvent.click(screen.getByRole('button', { name: 'Сохранить' }));

    await waitFor(() =>
      expect(mockApi.saveSystemSetting).toHaveBeenCalledWith('limits.max_upload_mb', 512),
    );
  });

  it('marks a restart-only setting read-only and explains the pending restart', async () => {
    mockApi.getSystemSettings.mockResolvedValue(
      response({
        values: { 'network.allowed_origins': ['https://saved.example'] },
        effective: { 'network.allowed_origins': ['https://running.example'] },
      }),
    );

    const { container } = renderGroup(['network.allowed_origins']);

    expect(await screen.findByText('Разрешённые источники (CORS)')).toBeTruthy();
    expect(screen.getByText('только чтение')).toBeTruthy();
    expect(screen.getByText('ожидает перезапуска')).toBeTruthy();
    expect(screen.queryByRole('button', { name: 'Сохранить' })).toBeNull();

    const hint = container.querySelector('.field__hint');
    expect(hint?.textContent).toContain('https://running.example');
    expect(hint?.textContent).toContain('https://saved.example');
  });
});
