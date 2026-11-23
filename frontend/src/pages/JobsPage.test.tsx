import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { JobsPage } from './JobsPage';
import { useAuth } from '../stores/auth';
import type { BackgroundJob, JobsResponse } from '../services/api';

vi.mock('../services/api', () => ({
  api: {
    listJobs: vi.fn(),
    pauseJob: vi.fn(),
    resumeJob: vi.fn(),
    cancelJob: vi.fn(),
    runMaintenance: vi.fn(),
  },
  errorMessage: (error: unknown) => (error instanceof Error ? error.message : 'ошибка'),
}));

vi.mock('../lib/realtime', () => ({
  realtime: {
    on: vi.fn(() => () => undefined),
  },
}));

const apiModule = await import('../services/api');
const mockApi = apiModule.api as unknown as {
  listJobs: ReturnType<typeof vi.fn>;
  pauseJob: ReturnType<typeof vi.fn>;
  resumeJob: ReturnType<typeof vi.fn>;
  cancelJob: ReturnType<typeof vi.fn>;
  runMaintenance: ReturnType<typeof vi.fn>;
};

function job(overrides: Partial<BackgroundJob>): BackgroundJob {
  return {
    id: 'job-1',
    jobType: 'system.objects.verify',
    status: 'pending',
    progress: 0,
    totalUnits: 2,
    doneUnits: 0,
    error: null,
    payload: {},
    cancellable: true,
    pauseRequested: false,
    pausedAt: null,
    createdAt: '2026-01-01T00:00:00Z',
    startedAt: null,
    finishedAt: null,
    actor: 'admin',
    ...overrides,
  };
}

const jobsResponse: JobsResponse = {
  items: [
    job({ id: 'job-pending', status: 'pending' }),
    job({ id: 'job-paused', status: 'paused', pausedAt: '2026-01-01T00:01:00Z' }),
  ],
  pending: 1,
  running: 0,
  paused: 1,
  failed: 0,
  completed: 0,
  maxConcurrent: 2,
};

function renderJobs() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <JobsPage />
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  vi.clearAllMocks();
  useAuth.setState({
    status: 'authenticated',
    error: null,
    session: {
      user: {
        id: 'admin',
        username: 'admin',
        displayName: 'Administrator',
        isSuperadmin: true,
        roles: [],
        permissions: [],
      },
      csrfToken: 'csrf',
      session: { expiresAt: '2026-12-01T00:00:00Z' },
      modules: {},
    },
  });
  mockApi.listJobs.mockResolvedValue(jobsResponse);
  mockApi.pauseJob.mockResolvedValue(job({ id: 'job-pending', status: 'paused' }));
  mockApi.resumeJob.mockResolvedValue(job({ id: 'job-paused', status: 'pending' }));
});

describe('JobsPage pause and resume', () => {
  it('pauses a pending job through the row action', async () => {
    renderJobs();

    const pause = await screen.findByRole('button', { name: 'Приостановить' });
    fireEvent.click(pause);

    await waitFor(() => expect(mockApi.pauseJob).toHaveBeenCalledWith('job-pending'));
    expect(mockApi.resumeJob).not.toHaveBeenCalled();
  });

  it('resumes a paused job and does not offer pause for it', async () => {
    renderJobs();

    const resume = await screen.findByRole('button', { name: 'Продолжить' });
    expect(screen.getAllByRole('button', { name: 'Приостановить' })).toHaveLength(1);

    fireEvent.click(resume);

    await waitFor(() => expect(mockApi.resumeJob).toHaveBeenCalledWith('job-paused'));
    expect(mockApi.pauseJob).not.toHaveBeenCalled();
  });
});
