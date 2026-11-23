import { request } from '@playwright/test';

import { ADMIN_PASSWORD, ADMIN_STATE, ADMIN_USERNAME, BASE_URL } from './fixtures/config';
import { apiData, apiFetch, openApiSession, uniqueTag, type ApiSession } from './fixtures/api';
import { expect, test } from './fixtures/test';

test.use({ storageState: ADMIN_STATE });

interface BuildRow {
  id: string;
  slug: string;
  name: string;
}

interface ReleaseRow {
  id: string;
  version: string;
}

interface Manifest {
  schema_version: string;
  build: { slug: string; release: string };
  files: Array<{
    id: string;
    path: string;
    size: number;
    sha256: string;
    download_url: string;
  }>;
}

test.describe('I1: Launcher API', () => {
  let admin: ApiSession;
  let buildId = '';
  let tokenName = '';

  test.beforeEach(async () => {
    admin = await openApiSession(ADMIN_STATE);
  });

  test.afterEach(async () => {
    try {
      if (tokenName) {
        const tokens = await apiData<Array<{ id: string; name: string }>>(
          admin,
          'GET',
          '/admin/launcher/tokens',
        );
        for (const token of tokens.filter((item) => item.name === tokenName)) {
          await apiFetch(admin, 'DELETE', `/admin/launcher/tokens/${token.id}`);
        }
      }
      if (buildId) {
        await apiFetch(admin, 'POST', `/minecraft/builds/${buildId}/archive`, { json: {} });
        await apiFetch(admin, 'DELETE', `/minecraft/builds/${buildId}`);
      }
    } finally {
      await admin.ctx.dispose();
    }
  });

  test('токен, каталог, манифест и скачивание с Range', async ({ page }) => {
    test.setTimeout(180_000);
    const tag = uniqueTag('lch-');
    const slug = `e2e-${tag}`;
    const buildName = `E2E Launcher ${tag}`;
    const fileName = `mods/${tag}.jar`;
    const payload = Buffer.from(`NEXUS launcher artifact ${tag}\n`, 'utf8');
    tokenName = `E2E token ${tag}`;

    const build = await apiData<BuildRow>(admin, 'POST', '/minecraft/builds', {
      json: {
        name: buildName,
        slug,
        description: 'E2E launcher fixture',
        version: '1.0.0',
        category: 'other',
        visibility: 'public',
        minecraftVersion: '1.20.1',
        loaderType: 'vanilla',
        loaderVersion: '',
        javaMajor: 17,
        javaMin: 17,
      },
    });
    buildId = build.id;
    expect(build.slug).toBe(slug);

    const upload = await apiData<{ id: string }>(admin, 'POST', '/minecraft/uploads', {
      json: {
        buildId,
        path: fileName,
        size: payload.length,
        category: 'mods',
        required: true,
        contentType: 'application/java-archive',
      },
    });
    const chunk = await apiFetch(admin, 'PATCH', `/minecraft/uploads/${upload.id}`, {
      body: payload,
      headers: {
        'Content-Type': 'application/offset+octet-stream',
        'Upload-Offset': '0',
      },
    });
    expect(chunk.status()).toBe(200);
    await apiData(admin, 'POST', `/minecraft/uploads/${upload.id}/complete`, {
      json: {},
      headers: { 'Idempotency-Key': `mc-${tag}` },
    });

    const release = await apiData<ReleaseRow>(
      admin,
      'POST',
      `/minecraft/builds/${buildId}/releases`,
      { json: { version: '1.0.0', changelog: `E2E launcher ${tag}` } },
    );
    await apiData(admin, 'POST', `/minecraft/builds/${buildId}/releases/${release.id}/publish`, {
      json: { acceptWarnings: true },
    });

    // Выдача токена через UI.
    await page.goto('/workspace/launcher');
    await expect(page.getByRole('heading', { name: 'Launcher API' })).toBeVisible();
    await page.getByRole('button', { name: 'Выдать токен' }).click();

    const issueDialog = page.getByRole('dialog');
    await expect(issueDialog).toBeVisible();
    await issueDialog.locator('#token-name').fill(tokenName);
    await issueDialog.locator('#token-user').fill(ADMIN_USERNAME);
    await issueDialog.locator('#token-password').fill(ADMIN_PASSWORD);
    await issueDialog.getByRole('button', { name: 'Выдать токен' }).click();

    const issuedDialog = page.getByRole('dialog').filter({ hasText: 'Токен создан' });
    await expect(issuedDialog).toBeVisible({ timeout: 30_000 });
    const token = await issuedDialog.locator('input[readonly]').inputValue();
    expect(token).toMatch(/^nxl_/);
    await issuedDialog.getByRole('button', { name: 'Готово' }).click();

    // Проверка токена из того же UI.
    await page.locator('#probe-token').fill(token);
    await page.getByRole('button', { name: 'Проверить каталог', exact: true }).click();
    const probe = page.locator('pre.cloud-preview-text').filter({ hasText: 'Каталог доступен:' });
    await expect(probe).toContainText(buildName, { timeout: 30_000 });

    // Тот же контракт напрямую, как у внешнего лаунчера.
    const launcher = await request.newContext({
      baseURL: BASE_URL,
      extraHTTPHeaders: {
        Accept: 'application/json',
        Authorization: `Bearer ${token}`,
      },
    });
    try {
      const catalogResponse = await launcher.get('/api/v1/launcher/catalog');
      expect(catalogResponse.status()).toBe(200);
      const catalog = (await catalogResponse.json()) as {
        data: { count: number; builds: Array<{ slug: string; name: string }> };
      };
      expect(catalog.data.count).toBeGreaterThan(0);
      expect(catalog.data.builds.some((item) => item.slug === slug)).toBe(true);

      const manifestResponse = await launcher.get(
        `/api/v1/launcher/builds/${slug}/latest/manifest`,
      );
      expect(manifestResponse.status()).toBe(200);
      const manifest = ((await manifestResponse.json()) as { data: Manifest }).data;
      expect(manifest.schema_version).toBe('1.0');
      expect(manifest.build.slug).toBe(slug);
      expect(manifest.build.release).toBe('1.0.0');
      expect(manifest.files).toHaveLength(1);
      const file = manifest.files[0];
      expect(file.path).toBe(fileName);
      expect(file.size).toBe(payload.length);

      const artifact = await launcher.get(`${BASE_URL}${file.download_url}`, {
        headers: { Range: 'bytes=0-15' },
      });
      expect(artifact.status()).toBe(206);
      expect(artifact.headers()['content-range']).toBe(`bytes 0-15/${payload.length}`);
      expect(artifact.headers()['x-file-sha256']).toBe(file.sha256);
      expect(Buffer.from(await artifact.body())).toEqual(payload.subarray(0, 16));
    } finally {
      await launcher.dispose();
    }
  });
});
