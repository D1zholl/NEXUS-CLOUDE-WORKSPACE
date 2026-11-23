import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { BuildFileManager } from './BuildFileManager';
import type { BuildFileRow } from '../services/types';

const listBuildFiles = vi.fn();
const toast = vi.fn();

vi.mock('../services/api', () => ({
  api: {
    listBuildFiles: (...args: unknown[]) => listBuildFiles(...args),
    updateBuildFile: vi.fn(),
    createBuildFolder: vi.fn(),
    deleteBuildFile: vi.fn(),
    startBuildUpload: vi.fn(),
    buildUploadStatus: vi.fn(),
    uploadBuildChunk: vi.fn(),
    completeBuildUpload: vi.fn(),
    abortBuildUpload: vi.fn(),
  },
  errorMessage: (error: unknown) => (error instanceof Error ? error.message : String(error)),
}));

vi.mock('../lib/realtime', () => ({
  realtime: {
    on: vi.fn(() => () => undefined),
  },
}));

vi.mock('../stores/ui', () => ({
  toast: (...args: unknown[]) => toast(...args),
}));

function row(path: string, kind: 'file' | 'dir' = 'file', size = 100): BuildFileRow {
  return {
    id: `id-${path}`,
    buildId: 'build-1',
    kind,
    path,
    category: path.split('/')[0] ?? 'other',
    side: 'client',
    required: true,
    enabled: true,
    description: '',
    objectId: 'object',
    sizeBytes: size,
    sha256: 'a'.repeat(64),
    createdAt: '2026-01-01T00:00:00Z',
    updatedAt: '2026-01-01T00:00:00Z',
  };
}

function renderManager() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <BuildFileManager
        buildId="build-1"
        canEdit
        canUpload
        chunkMaxBytes={1024 * 1024}
        maxUploadBytes={10 * 1024 * 1024}
        onChanged={() => undefined}
      />
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  listBuildFiles.mockResolvedValue({
    items: [
      row('options.txt'),
      row('mods', 'dir', 0),
      row('mods/create.jar', 'file', 200),
      row('config', 'dir', 0),
      row('config/settings.toml', 'file', 30),
      row('config/deep', 'dir', 0),
      row('config/deep/create.json', 'file', 40),
    ],
    stats: { files: 4, folders: 3, totalBytes: 270 },
  });
});

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe('BuildFileManager', () => {
  it('renders the canonical skeleton and navigates folders', async () => {
    renderManager();
    await screen.findByTestId('build-file-manager');
    await screen.findByTestId('build-tree-config');

    // Скелет виден даже там, где файлов ещё нет.
    for (const path of ['mods', 'shaderpacks', 'resourcepacks', 'config']) {
      expect(screen.getByTestId(`build-tree-${path}`)).toBeTruthy();
    }
    expect(screen.getByTestId('build-tree-options')).toBeTruthy();

    fireEvent.click(screen.getByTestId('build-tree-config'));
    await waitFor(() => expect(screen.getAllByText('config/settings.toml').length).toBeGreaterThan(0));
    expect(screen.getAllByText('deep').length).toBeGreaterThan(0);

    fireEvent.click(screen.getAllByText('deep')[0]);
    await waitFor(() => expect(screen.getAllByText('create.json').length).toBeGreaterThan(0));
  });

  it('keeps focus in the file settings field while typing', async () => {
    renderManager();
    await screen.findByTestId('build-tree-config');
    fireEvent.click(screen.getByTestId('build-tree-config'));

    const edit = await screen.findByRole('button', { name: 'Настроить' });
    fireEvent.click(edit);

    const field = (await screen.findByLabelText('Относительный путь')) as HTMLInputElement;
    expect(document.activeElement).toBe(field);
    const value = 'config/settings-long-name.toml';
    for (let index = 0; index < value.length; index += 1) {
      fireEvent.change(field, { target: { value: value.slice(0, index + 1) } });
      expect(document.activeElement).toBe(field);
    }
    expect(field.value).toBe(value);
  });

  it('filters files by search across the whole build', async () => {
    renderManager();
    await screen.findByTestId('build-file-manager');
    const search = screen.getByLabelText('Поиск файлов сборки');
    fireEvent.change(search, { target: { value: 'create.json' } });
    await waitFor(() => {
      const table = screen.getByRole('table');
      expect(within(table).getByText('create.json')).toBeTruthy();
      expect(within(table).queryByText('settings.toml')).toBeNull();
    });
  });
});
