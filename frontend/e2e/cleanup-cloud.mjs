/**
 * Удаляет все Cloud-записи тестового пользователя E2E.
 * Нужен после упавшего прогона, когда afterEach/тест не успел очистить
 * 2000-файловую папку. Работает только с E2E-пользователем.
 *
 * Использование (backend уже запущен): npm.cmd run e2e:cleanup
 */
const base = process.env.E2E_BASE_URL ?? `http://127.0.0.1:${process.env.E2E_PORT ?? 8091}`;
const username = process.env.E2E_USER_USERNAME ?? 'e2e-user';
const password = process.env.E2E_USER_PASSWORD ?? 'Nexus!E2E-user-2026';

const login = await fetch(`${base}/api/v1/auth/login`, {
  method: 'POST',
  headers: { 'Content-Type': 'application/json' },
  body: JSON.stringify({ username, password }),
});
if (!login.ok) {
  console.error(`[cleanup] login ${username}: HTTP ${login.status()} ${await login.text()}`);
  process.exit(1);
}
const cookie = (login.headers.get('set-cookie') ?? '').split(';')[0];
const csrf = (await login.json()).data.csrfToken;
const headers = {
  'Content-Type': 'application/json',
  'X-CSRF-Token': csrf,
  Cookie: cookie,
};

const roots = [];
for (let page = 1; page <= 100; page += 1) {
  const response = await fetch(
    `${base}/api/v1/cloud/entries?view=folder&perPage=100&page=${page}`,
    { headers: { Cookie: cookie } },
  );
  if (!response.ok) {
    console.error(`[cleanup] list: HTTP ${response.status()} ${await response.text()}`);
    process.exit(1);
  }
  const data = (await response.json()).data;
  roots.push(...data.items);
  if (data.items.length < 100) break;
}

if (roots.length === 0) {
  console.log('[cleanup] нечего удалять');
  process.exit(0);
}

for (const entry of roots) {
  const trash = await fetch(`${base}/api/v1/cloud/entries/${entry.id}`, {
    method: 'DELETE',
    headers,
  });
  const purge = await fetch(`${base}/api/v1/cloud/entries/${entry.id}/purge`, {
    method: 'DELETE',
    headers,
  });
  console.log(`[cleanup] ${entry.name}: trash=${trash.status} purge=${purge.status}`);
}
console.log(`[cleanup] удалено корневых записей: ${roots.length}`);
