import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { MemoryRouter } from 'react-router-dom';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { TransferDrawer } from './TransferDrawer';
import type { UploadItem } from '../lib/uploadQueue';
import type { CloudEntry } from '../services/api';
import { useTransfers } from '../stores/transfers';

vi.mock('../lib/realtime', () => ({
  realtime: {
    on: () => () => undefined,
  },
}));

function makeEntry(name: string): CloudEntry {
  return {
    id: 'entry-1',
    ownerId: 'user-1',
    parentId: null,
    name,
    entryType: 'file',
    sizeBytes: 100,
    contentType: 'application/octet-stream',
    starred: false,
    createdAt: '2026-11-21T10:00:00Z',
    updatedAt: '2026-11-21T10:00:00Z',
    trashedAt: null,
  };
}

function makeItem(patch: Partial<UploadItem> & Pick<UploadItem, 'key' | 'state'>): UploadItem {
  const { key, state, ...rest } = patch;
  const file = new File([new Uint8Array(100)], patch.entry?.name ?? `${key}.bin`);
  return {
    file,
    segments: [],
    groupName: null,
    parentId: null,
    size: 100,
    uploaded: state === 'done' ? 100 : 40,
    attempts: 0,
    error: null,
    entry: null,
    sessionId: null,
    completeKey: null,
    completeKeySessionId: null,
    samples: [],
    key,
    state,
    ...rest,
  };
}

function renderDrawer() {
  const client = new QueryClient();
  return render(
    <QueryClientProvider client={client}>
      <MemoryRouter>
        <TransferDrawer />
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

afterEach(() => {
  cleanup();
  useTransfers.setState({
    items: [],
    stats: null,
    collapsed: false,
    hidden: false,
    revealRequest: null,
  });
});

describe('TransferDrawer', () => {
  it('shows one grouped container with a completed file and an error', () => {
    const items = [
      makeItem({ key: 'done', state: 'done', entry: makeEntry('report.pdf') }),
      makeItem({ key: 'failed', state: 'failed', error: 'Сеть недоступна' }),
    ];
    useTransfers.setState({
      items,
      stats: {
        totalBytes: 200,
        uploadedBytes: 140,
        progress: 0.7,
        speedBytesPerSec: 2048,
        etaSeconds: 30,
        active: 0,
        queued: 0,
        failed: 1,
        conflicts: 0,
        done: 1,
        groups: [
          {
            name: 'Docs',
            totalBytes: 200,
            uploadedBytes: 140,
            progress: 0.7,
            total: 2,
            done: 1,
            failed: 1,
            active: 0,
            queued: 0,
          },
        ],
      },
    });

    renderDrawer();

    expect(screen.getByTestId('upload-queue')).toBeTruthy();
    expect(screen.getByText(/Загрузка: 1 из 2 · есть ошибки/)).toBeTruthy();
    expect(screen.getByText(/Папка «Docs»/)).toBeTruthy();
    expect(screen.getByText('report.pdf')).toBeTruthy();
    expect(screen.getByText('Сеть недоступна')).toBeTruthy();
    expect(screen.getByRole('button', { name: /В папке/ })).toBeTruthy();
    // Окно с ошибкой не скрывается само.
    expect(screen.getByText(/Окно не закроется автоматически/)).toBeTruthy();
  });

  it('collapses to a compact bar and keeps the overall progress', () => {
    useTransfers.setState({
      items: [makeItem({ key: 'active', state: 'uploading' })],
      stats: {
        totalBytes: 100,
        uploadedBytes: 40,
        progress: 0.4,
        speedBytesPerSec: 1024,
        etaSeconds: 60,
        active: 1,
        queued: 0,
        failed: 0,
        conflicts: 0,
        done: 0,
        groups: [],
      },
    });

    renderDrawer();
    expect(screen.getByText('40%')).toBeTruthy();

    fireEvent.click(screen.getByRole('button', { name: 'Свернуть очередь' }));
    expect(useTransfers.getState().collapsed).toBe(true);
    expect(screen.queryByText('Очистить завершённые')).toBeNull();
    // Компактная плашка сохраняет общий прогресс.
    expect(screen.getByText('40%')).toBeTruthy();
  });

  it('requires explicit confirmation before hiding a failed transfer', () => {
    useTransfers.setState({
      items: [makeItem({ key: 'failed', state: 'failed', error: 'Таймаут' })],
      stats: {
        totalBytes: 100,
        uploadedBytes: 40,
        progress: 0.4,
        speedBytesPerSec: 0,
        etaSeconds: null,
        active: 0,
        queued: 0,
        failed: 1,
        conflicts: 0,
        done: 0,
        groups: [],
      },
    });

    renderDrawer();
    fireEvent.click(screen.getByRole('button', { name: 'Закрыть очередь загрузок' }));

    expect(screen.getByText('Скрыть окно с ошибками?')).toBeTruthy();
    fireEvent.click(screen.getByRole('button', { name: 'Скрыть' }));

    expect(useTransfers.getState().hidden).toBe(true);
    expect(useTransfers.getState().items).toHaveLength(1);
    expect(screen.queryByTestId('upload-queue')).toBeNull();
  });
});
