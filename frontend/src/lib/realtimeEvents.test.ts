import { describe, expect, it } from 'vitest';

import type { BackgroundJob, JobsResponse } from '../services/api';
import {
  applyDownloadEvent,
  applyJobEvent,
  dismissDownload,
  type DownloadItem,
} from './realtimeEvents';
import type { RealtimeEvent } from './realtime';

function job(patch: Partial<BackgroundJob> = {}): BackgroundJob {
  return {
    id: 'job-1',
    jobType: 'system.storage.check',
    status: 'running',
    progress: 10,
    totalUnits: 100,
    doneUnits: 10,
    error: null,
    payload: {},
    cancellable: true,
    pauseRequested: false,
    pausedAt: null,
    createdAt: '2026-11-19T09:00:00Z',
    startedAt: '2026-11-19T09:00:01Z',
    finishedAt: null,
    actor: 'admin',
    ...patch,
  };
}

function jobsResponse(...items: BackgroundJob[]): JobsResponse {
  return {
    items,
    pending: 0,
    running: 1,
    paused: 0,
    failed: 0,
    completed: 0,
    maxConcurrent: 2,
  };
}

function event(
  patch: Partial<RealtimeEvent> & { kind: string; payload: Record<string, unknown> },
): RealtimeEvent {
  return { userId: null, at: '2026-11-19T09:00:02Z', ...patch };
}

describe('job realtime events', () => {
  it('updates progress of the addressed job only', () => {
    const data = jobsResponse(job(), job({ id: 'job-2', progress: 5 }));
    const next = applyJobEvent(
      data,
      event({
        kind: 'job.progress',
        payload: { jobId: 'job-1', done: 40, total: 100, progress: 40 },
      }),
    );
    expect(next.items[0]).toMatchObject({ progress: 40, doneUnits: 40, totalUnits: 100 });
    expect(next.items[1].progress).toBe(5);
    expect(next.pending).toBe(data.pending);
  });

  it('applies paused, resumed and terminal statuses', () => {
    const paused = applyJobEvent(
      jobsResponse(job()),
      event({ kind: 'job.paused', payload: { jobId: 'job-1', pausedAt: '2026-11-19T09:05:00Z' } }),
    );
    expect(paused.items[0]).toMatchObject({
      status: 'paused',
      pauseRequested: false,
      pausedAt: '2026-11-19T09:05:00Z',
    });

    const resumed = applyJobEvent(
      paused,
      event({ kind: 'job.resumed', payload: { jobId: 'job-1' } }),
    );
    expect(resumed.items[0]).toMatchObject({ status: 'pending', pausedAt: null });

    const completed = applyJobEvent(
      resumed,
      event({ kind: 'job.completed', payload: { jobId: 'job-1', jobType: 'x', progress: 100 } }),
    );
    expect(completed.items[0]).toMatchObject({ status: 'completed', progress: 100 });

    const failed = applyJobEvent(
      completed,
      event({ kind: 'job.failed', payload: { jobId: 'job-1', jobType: 'x', error: 'boom' } }),
    );
    expect(failed.items[0]).toMatchObject({ status: 'failed', error: 'boom' });

    const cancelled = applyJobEvent(
      failed,
      event({ kind: 'job.cancelled', payload: { jobId: 'job-1', jobType: 'x' } }),
    );
    expect(cancelled.items[0].status).toBe('cancelled');
  });

  it('ignores events for jobs outside the loaded page', () => {
    const data = jobsResponse(job());
    expect(applyJobEvent(data, event({ kind: 'job.progress', payload: { jobId: 'other' } }))).toBe(
      data,
    );
  });
});

describe('download realtime events', () => {
  it('creates, updates, completes and fails a download entry', () => {
    let items: DownloadItem[] = [];
    items = applyDownloadEvent(
      items,
      event({
        kind: 'download.progress',
        payload: { entryId: 'e1', name: 'a.bin', transferred: 0, size: 100, percent: 0 },
      }),
    );
    expect(items).toHaveLength(1);
    expect(items[0]).toMatchObject({ entryId: 'e1', state: 'active', percent: 0 });

    items = applyDownloadEvent(
      items,
      event({
        kind: 'download.progress',
        payload: { entryId: 'e1', name: 'a.bin', transferred: 50, size: 100, percent: 50 },
      }),
    );
    expect(items[0]).toMatchObject({ transferred: 50, percent: 50, state: 'active' });

    items = applyDownloadEvent(
      items,
      event({
        kind: 'download.completed',
        payload: { entryId: 'e1', name: 'a.bin', transferred: 100, size: 100, percent: 100 },
      }),
    );
    expect(items[0]).toMatchObject({ state: 'done', percent: 100 });

    items = applyDownloadEvent(
      items,
      event({
        kind: 'download.failed',
        payload: {
          entryId: 'e2',
          name: 'b.bin',
          transferred: 10,
          size: 100,
          percent: 10,
          code: 'DOWNLOAD_INTERRUPTED',
          message: 'Соединение прервано.',
        },
      }),
    );
    expect(items[0]).toMatchObject({
      entryId: 'e2',
      state: 'failed',
      code: 'DOWNLOAD_INTERRUPTED',
      message: 'Соединение прервано.',
    });
    expect(items).toHaveLength(2);
  });

  it('dismisses finished entries', () => {
    const items = applyDownloadEvent(
      [],
      event({
        kind: 'download.completed',
        payload: { entryId: 'e1', name: 'a', transferred: 1, size: 1, percent: 100 },
      }),
    );
    expect(dismissDownload(items, 'e1')).toEqual([]);
  });

  it('ignores unrelated events and malformed payloads', () => {
    const items: DownloadItem[] = [];
    expect(
      applyDownloadEvent(items, event({ kind: 'upload.progress', payload: { entryId: 'e1' } })),
    ).toBe(items);
    expect(
      applyDownloadEvent(items, event({ kind: 'download.progress', payload: { name: 'x' } })),
    ).toBe(items);
  });
});
