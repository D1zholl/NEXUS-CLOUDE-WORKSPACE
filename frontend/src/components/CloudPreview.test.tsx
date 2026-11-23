import { render, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { CloudPreviewModal } from './CloudPreview';
import type { CloudEntry } from '../services/api';
import { PREVIEW_MAX_BYTES, PREVIEW_TEXT_MAX_BYTES } from '../lib/preview';

function entry(name: string, sizeBytes: number): CloudEntry {
  return {
    id: '11111111-1111-1111-1111-111111111111',
    ownerId: '22222222-2222-2222-2222-222222222222',
    parentId: null,
    name,
    entryType: 'file',
    sizeBytes,
    contentType: 'application/octet-stream',
    starred: false,
    createdAt: '2026-01-01T00:00:00Z',
    updatedAt: '2026-01-01T00:00:00Z',
    trashedAt: null,
  };
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe('CloudPreviewModal', () => {
  it('shows a clear warning for a PDF over 100 MB and does not fetch it', () => {
    const fetchMock = vi.fn();
    vi.stubGlobal('fetch', fetchMock);

    render(
      <CloudPreviewModal
        entry={entry('large.pdf', PREVIEW_MAX_BYTES + 1)}
        onClose={() => undefined}
      />,
    );

    expect(screen.getByText(/Предпросмотр ограничен 100 МБ/)).toBeTruthy();
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it('renders <script> from a code file as text, not as executable markup', async () => {
    const source = '<script>alert("xss")</script>';
    vi.stubGlobal(
      'fetch',
      vi.fn(async () => ({
        ok: true,
        headers: new Headers({ 'content-length': String(source.length) }),
        text: async () => source,
      })),
    );

    const { container } = render(
      <CloudPreviewModal entry={entry('danger.js', source.length)} onClose={() => undefined} />,
    );

    await waitFor(() => {
      expect(screen.getByRole('dialog').textContent).toContain(source);
    });

    expect(container.querySelector('script')).toBeNull();
    expect(container.querySelector('iframe')).toBeNull();
  });

  it('stops reading text when content-length is over the 512 KB limit', async () => {
    const text = vi.fn(async () => 'should not be read');
    vi.stubGlobal(
      'fetch',
      vi.fn(async () => ({
        ok: true,
        headers: new Headers({ 'content-length': String(PREVIEW_TEXT_MAX_BYTES + 1) }),
        text,
      })),
    );

    render(
      <CloudPreviewModal
        entry={entry('big.log', PREVIEW_TEXT_MAX_BYTES + 1)}
        onClose={() => undefined}
      />,
    );

    expect(await screen.findByText(/Предпросмотр текста ограничен 512 КБ/)).toBeTruthy();
    expect(text).not.toHaveBeenCalled();
  });
});
