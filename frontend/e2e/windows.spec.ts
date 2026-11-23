import fs from 'node:fs';
import path from 'node:path';

import { expect, test } from './fixtures/test';
import { BASE_URL, USER_STATE, readMeta } from './fixtures/config';
import {
  apiData,
  apiFetch,
  apiPurgeEntry,
  decodeFilenameStar,
  expectFailure,
  openApiSession,
  uniqueTag,
  type ApiSession,
} from './fixtures/api';
import {
  cardEntryId,
  gotoSignedInCloud,
  openCloudCard,
  purgeCard,
  uploadFolder,
  waitForCloudCard,
  waitForQueueDone,
} from './fixtures/cloud';

interface CloudEntry {
  id: string;
  name: string;
  sizeBytes: number;
}

async function startUpload(session: ApiSession, name: string, payload: Buffer): Promise<string> {
  const created = await apiData<{ id: string }>(session, 'POST', '/cloud/uploads', {
    json: {
      name,
      parentId: null,
      size: payload.length,
      contentType: 'text/plain',
    },
  });
  const chunk = await apiFetch(session, 'PATCH', `/cloud/uploads/${created.id}`, {
    body: payload,
    headers: {
      'Content-Type': 'application/offset+octet-stream',
      'Upload-Offset': '0',
    },
  });
  expect(chunk.status()).toBe(200);
  return created.id;
}

async function completeUpload(session: ApiSession, id: string, tag: string): Promise<CloudEntry> {
  return apiData<CloudEntry>(session, 'POST', `/cloud/uploads/${id}/complete`, {
    json: { strategy: 'auto' },
    headers: { 'Idempotency-Key': `win-${tag}` },
  });
}

async function createFolder(
  session: ApiSession,
  name: string,
  parentId: string | null,
): Promise<{ id: string }> {
  return apiData<{ id: string }>(session, 'POST', '/cloud/entries', {
    json: { name, parentId },
  });
}

test.describe('I1: Windows/NTFS', () => {
  test.skip(process.platform !== 'win32', 'Сценарии рассчитаны на Windows/NTFS');

  test('загрузка файла с не-ASCII именем и корректный filename*', async () => {
    const session = await openApiSession(USER_STATE);
    const tag = uniqueTag('uni-');
    const name = `Отчёт ${tag} (итог).txt`;
    const payload = Buffer.from(`NEXUS ЭПСИЛОН ${tag}\n`, 'utf8');
    let entryId = '';
    try {
      const uploadId = await startUpload(session, name, payload);
      const entry = await completeUpload(session, uploadId, tag);
      entryId = entry.id;
      expect(entry.name).toBe(name);
      expect(entry.sizeBytes).toBe(payload.length);

      const download = await apiFetch(
        session,
        'GET',
        `/cloud/entries/${encodeURIComponent(entryId)}/download`,
      );
      expect(download.status()).toBe(200);
      const disposition = download.headers()['content-disposition'] ?? '';
      expect(disposition).toContain("filename*=UTF-8''");
      expect(decodeFilenameStar(disposition)).toBe(name);
      expect(Buffer.from(await download.body())).toEqual(payload);
    } finally {
      if (entryId) await apiPurgeEntry(session, entryId);
      await session.ctx.dispose();
    }
  });

  test('имена, отличающиеся только регистром, не перезаписывают файл', async () => {
    const session = await openApiSession(USER_STATE);
    const tag = uniqueTag('case-');
    const originalName = `Report-${tag}.TXT`;
    const duplicateName = originalName.toLowerCase();
    const original = Buffer.from(`original ${tag}\n`, 'utf8');
    let entryId = '';
    let secondUpload = '';
    try {
      const firstUpload = await startUpload(session, originalName, original);
      const entry = await completeUpload(session, firstUpload, tag);
      entryId = entry.id;
      expect(entry.name).toBe(originalName);

      secondUpload = await startUpload(
        session,
        duplicateName,
        Buffer.from(`attacker ${tag}\n`, 'utf8'),
      );
      const conflict = await apiFetch(session, 'POST', `/cloud/uploads/${secondUpload}/complete`, {
        json: { strategy: 'auto' },
        headers: { 'Idempotency-Key': `win-case-${tag}` },
      });
      const failure = await expectFailure(conflict, 409);
      expect(failure.body.error.code).toBe('FILE_NAME_CONFLICT');
      expect(failure.body.error.details).toMatchObject({ existingFileId: entryId });

      const listing = await apiData<{ items: CloudEntry[] }>(
        session,
        'GET',
        `/cloud/entries?view=folder&perPage=100&q=${encodeURIComponent(tag)}`,
      );
      const same = listing.items.filter(
        (item) => item.name.toLowerCase() === originalName.toLowerCase(),
      );
      expect(same).toHaveLength(1);
      expect(same[0].id).toBe(entryId);
      expect(same[0].sizeBytes).toBe(original.length);

      const download = await apiFetch(
        session,
        'GET',
        `/cloud/entries/${encodeURIComponent(entryId)}/download`,
      );
      expect(download.status()).toBe(200);
      expect(Buffer.from(await download.body())).toEqual(original);
    } finally {
      if (secondUpload) {
        await apiFetch(session, 'DELETE', `/cloud/uploads/${secondUpload}`);
      }
      if (entryId) await apiPurgeEntry(session, entryId);
      await session.ctx.dispose();
    }
  });

  test('слишком длинный путь отклоняется с PATH_TOO_LONG', async () => {
    const session = await openApiSession(USER_STATE);
    const tag = uniqueTag('lp-');
    const short = tag.slice(-3).toLowerCase();
    // Короткий корень оставляет бюджет пути: 5 + 1 + 21 * 10 + 20 = 236.
    const root = await createFolder(session, `r${tag.slice(-4).toLowerCase()}`, null);
    try {
      let parentId = root.id;
      // 21 уровень по 10 UTF-16 единиц: 230 единиц + 20 слэшей.
      for (let index = 0; index < 21; index += 1) {
        const name = `L${short}${String(index).padStart(2, '0')}abcd`;
        expect(name.length).toBe(10);
        const created = await createFolder(session, name, parentId);
        parentId = created.id;
      }

      const leaf = 'я'.repeat(128);
      const tooLong = await apiFetch(session, 'POST', '/cloud/entries', {
        json: { name: leaf, parentId },
      });
      const failure = await expectFailure(tooLong, 422);
      expect(failure.body.error.code).toBe('PATH_TOO_LONG');
      const details = failure.body.error.details as {
        actualLength: number;
        maxLength: number;
      };
      expect(details.actualLength).toBeGreaterThan(260);
      expect(details.actualLength).toBeGreaterThan(details.maxLength);

      const single = await apiFetch(session, 'POST', '/cloud/entries', {
        json: { name: 'я'.repeat(129) },
      });
      const singleFailure = await expectFailure(single, 422);
      expect(singleFailure.body.error.code).toBe('VALIDATION_ERROR');
    } finally {
      await apiPurgeEntry(session, root.id);
      await session.ctx.dispose();
    }
  });

  test('обрыв после части файла продолжается с серверного offset', async () => {
    const session = await openApiSession(USER_STATE);
    const tag = uniqueTag('resume-');
    const payload = Buffer.alloc(64 * 1024);
    for (let index = 0; index < payload.length; index += 1) {
      payload[index] = index % 251;
    }
    let entryId = '';
    try {
      const uploadId = await apiData<{ id: string }>(session, 'POST', '/cloud/uploads', {
        json: {
          name: `resume-${tag}.bin`,
          parentId: null,
          size: payload.length,
          contentType: 'application/octet-stream',
        },
      });

      const firstPart = payload.subarray(0, 16 * 1024);
      const firstChunk = await apiFetch(session, 'PATCH', `/cloud/uploads/${uploadId.id}`, {
        body: firstPart,
        headers: {
          'Content-Type': 'application/offset+octet-stream',
          'Upload-Offset': '0',
        },
      });
      expect(firstChunk.status()).toBe(200);

      // Эмуляция обрыва: продолжение идёт из нового клиентского контекста,
      // как после перезапуска службы. Сервер обязан отдать фактический offset.
      const resumedSession = await openApiSession(USER_STATE);
      try {
        const status = await apiData<{ offset: number; status: string }>(
          resumedSession,
          'GET',
          `/cloud/uploads/${uploadId.id}`,
        );
        expect(status.status).toBe('active');
        expect(status.offset).toBe(firstPart.length);

        const rest = payload.subarray(status.offset);
        const patch = await apiFetch(resumedSession, 'PATCH', `/cloud/uploads/${uploadId.id}`, {
          body: rest,
          headers: {
            'Content-Type': 'application/offset+octet-stream',
            'Upload-Offset': String(status.offset),
          },
        });
        expect(patch.status()).toBe(200);

        const entry = await completeUpload(resumedSession, uploadId.id, tag);
        entryId = entry.id;
        expect(entry.sizeBytes).toBe(payload.length);

        const download = await apiFetch(
          resumedSession,
          'GET',
          `/cloud/entries/${entryId}/download`,
        );
        expect(download.status()).toBe(200);
        expect(Buffer.from(await download.body())).toEqual(payload);
      } finally {
        await resumedSession.ctx.dispose();
      }
    } finally {
      if (entryId) await apiPurgeEntry(session, entryId);
      await session.ctx.dispose();
    }
  });

  test.describe('возобновление после обрыва', () => {
    test.use({ storageState: USER_STATE });

    test('обрыв канала во время чанка продолжается с серверного offset', async ({ page }) => {
      test.setTimeout(300_000);
      const meta = readMeta();
      const rootName = path.basename(meta.files.pauseDir);
      const fileName = path.basename(meta.files.pauseFile);
      const session = await openApiSession(USER_STATE);
      const config = await apiData<{ chunkMaxBytes: number }>(session, 'GET', '/cloud/config');
      const chunkSize = config.chunkMaxBytes;
      expect(meta.files.largeFileBytes).toBeGreaterThan(chunkSize);

      const offsets: number[] = [];
      let aborted = false;
      await page.route('**/api/v1/cloud/uploads/*', async (route) => {
        const request = route.request();
        if (request.method() === 'PATCH') {
          const offset = Number(request.headers()['upload-offset'] ?? '-1');
          offsets.push(offset);
          if (!aborted && offset > 0) {
            aborted = true;
            await route.abort('connectionreset');
            return;
          }
        }
        try {
          await route.continue();
        } catch {
          // Навигация могла уже обработать запрос — для теста это не ошибка.
        }
      });

      try {
        await gotoSignedInCloud(page);
        // Папка идёт через UploadQueue: именно она умеет продолжать с offset.
        await uploadFolder(page, meta.files.pauseDir);

        const row = page.getByTestId('upload-queue-item').filter({ hasText: fileName });
        await expect(row).toBeVisible({ timeout: 30_000 });
        await waitForQueueDone(page, 1, 240_000);
        expect(aborted).toBe(true);

        // Первый чанк — с нуля, оборванный и повторный — с одного и того же
        // серверного смещения, без повторной отправки начала файла.
        expect(offsets[0]).toBe(0);
        const resumed = offsets.filter((offset) => offset === chunkSize);
        expect(resumed.length).toBeGreaterThanOrEqual(2);

        await waitForCloudCard(page, rootName);
        await openCloudCard(page, rootName);
        await waitForCloudCard(page, fileName);
        const entryId = await cardEntryId(page, fileName);

        const size = meta.files.largeFileBytes;
        const tail = await page.request.get(
          `${BASE_URL}/api/v1/cloud/entries/${entryId}/download`,
          { headers: { Range: `bytes=${size - 16}-${size - 1}` } },
        );
        expect(tail.status()).toBe(206);
        expect(tail.headers()['content-range']).toBe(`bytes ${size - 16}-${size - 1}/${size}`);
        const bytes = Buffer.from(await tail.body());
        expect(bytes.length).toBe(16);
        // Фикстура заполнена байтом 0xA5; хвост подтверждает целостность склейки.
        expect(bytes).toEqual(Buffer.alloc(16, 0xa5));

        // Сверяем реальный локальный размер без чтения 70 МБ в память.
        const stat = fs.statSync(meta.files.pauseFile);
        expect(stat.size).toBe(size);

        await page.goto('/workspace/storage');
        await purgeCard(page, rootName);
      } finally {
        await session.ctx.dispose();
      }
    });
  });
});
