# J2. Caddy for Windows: HTTPS, WebSocket, Range, trust proxy

Дата: 20.11.2026, 12:24–12:33 MSK.  
Стенд: Windows 11 Pro 10.0.26300 x64, 15,7 ГиБ RAM, PostgreSQL 17.12,
`nexus.exe` 0.1.0 (release), служба `NexusWorkspace` (NT SERVICE\NexusWorkspace).

## 1. Что развёрнуто

| Компонент | Факт |
|---|---|
| Caddy | **v2.11.9** `h1:bhZtcRKQo…`, `C:\Caddy\caddy.exe` |
| Обёртка службы | **NSSM 2.24**, служба `Caddy`, `AUTO_START`, LocalSystem |
| binPath | `C:\Caddy\nssm.exe`, AppParameters `run --config C:\Caddy\Caddyfile --adapter caddyfile` |
| Порты | 80, 443 (`::`), admin API `127.0.0.1:2019` |
| Сертификат | внутренний CA Caddy, `CN=Caddy Local Authority - 2026 ECC Root`, thumbprint `15A9100D8327DF2AA4AA4D9CD0E14AEC54C6EF2B` |
| SAN | `localhost`, `172.24.241.69` (LAN-адрес стенда) |
| Доверие CA | `certutil -addstore -f Root C:\Caddy\data\pki\authorities\local\root.crt` |
| Firewall | правила `NEXUS WORKSPACE - HTTP 80`, `NEXUS WORKSPACE - HTTPS 443` |

`caddy trust` из-под LocalSystem завершился ошибкой
`failed to install root certificate: add cert failed: The request is not supported`,
поэтому корневой сертификат установлен явно через `certutil` в
`LocalMachine\Root`. После этого HTTPS проверяется штатной валидацией .NET
без отключения проверки сертификата.

Ключевые директивы `C:\Caddy\Caddyfile` (шаблон — `deployment/Caddyfile.windows`):

```caddyfile
{
    local_certs
    admin 127.0.0.1:2019
    storage file_system C:/Caddy/data
}

http://localhost, http://172.24.241.69 {
    redir https://{host}{uri} permanent
}

https://localhost, https://172.24.241.69 {
    tls internal
    header {
        Strict-Transport-Security "max-age=31536000; includeSubDomains"
        X-Content-Type-Options "nosniff"
        -Server
    }
    request_body { max_size 11GB }
    reverse_proxy 127.0.0.1:8080 {
        header_up X-Real-IP {remote_host}
        header_up X-Forwarded-For {remote_host}   # перезапись, не дополнение
        header_up X-Forwarded-Proto {scheme}
        transport http {
            dial_timeout 10s
            response_header_timeout 5m
            read_timeout 30m
            write_timeout 30m
        }
    }
}
```

`request_body max_size 11GB` выше лимита приложения (10 ГиБ); чанки 1 ГиБ
проходят целиком. Таймауты транспорта 30 минут рассчитаны на длинные загрузки
и Range-ответы.

## 2. HTTPS, редирект, HSTS

```text
$ curl -sS -o NUL -D - http://localhost/api/v1/health
HTTP/1.1 301 Moved Permanently
Location: https://localhost/api/v1/health
Server: Caddy

$ curl -sS -o NUL -D - --ssl-no-revoke https://172.24.241.69/api/v1/health
HTTP/2 200
strict-transport-security: max-age=31536000; includeSubDomains
x-content-type-options: nosniff
```

Вход по HTTPS: `POST /api/v1/auth/login` → 200, `csrfToken` получен,
`GET /api/v1/auth/me` → 200. Cookie `nexus_session` выставлена с `Secure`
(`NEXUS_COOKIE_SECURE=true`), CSRF-проверка работает.

## 3. Приёмка J2 (20/20 PASS)

Скрипт: `tests/proxy-acceptance.ps1`. Полный JSON:
[`logs/j2-results.json`](logs/j2-results.json), консоль:
[`logs/j2-run.log`](logs/j2-run.log).

| Проверка | Факт |
|---|---|
| HTTPS login / CSRF / `/auth/me` | 200 / 200 / 200 |
| Secure cookie | `nexus_session` |
| HSTS | `max-age=31536000; includeSubDomains` |
| `Server` скрыт | заголовок отсутствует |
| WebSocket Upgrade | `wss://localhost/api/v1/ws`, `101` |
| WS `connected` | `{"activeConnections":1,"kind":"connected","system":true,...}` |
| Сессия загрузки | `chunkMaxBytes=67108864`, 64 чанка по 16 МиБ |
| Пауза 5 с | offset `268435456 → 268435456` |
| Устаревший offset | `409 UPLOAD_OFFSET_MISMATCH` |
| Загрузка 1 ГиБ | offset = 1073741824 за **28,0 с** |
| WS `upload.completed` | событие получено через Caddy |
| Range | **11/11** ответов `206`, `Accept-Ranges: bytes` |
| Range 1 ГиБ | SHA-256 совпал за **20,4 с** |
| LAN-вход | `https://172.24.241.69` → 200 |
| IP в аудите | `172.24.241.69`, не `127.0.0.1` |
| Подмена `X-Forwarded-For` | `203.0.113.77` в аудите отсутствует |

Файл: 1 ГиБ, SHA-256
`4bfe67e410f7b79d9182b6bb39eb538dbe566cce3c4d66cf171b5a01cf4dc957`.
Запись в хранилище: `f174f102-1060-4267-a6ba-9c70d7906dcb`.

Пример Range-ответа:

```text
Range 0:  0-104857599          HTTP 206 Content-Range=bytes 0-104857599/1073741824
Range 10: 1048576000-1073741823 HTTP 206 Content-Range=bytes 1048576000-1073741823/1073741824
```

## 4. `trust_proxy`: только за Caddy

Сценарий A — через Caddy, `NEXUS_TRUST_PROXY=true`: вход по
`https://172.24.241.69` с заголовком `X-Forwarded-For: 203.0.113.77`.
Caddy перезаписал заголовок адресом соединения, аудит:
`event=auth.login.success ip=172.24.241.69 actor=j2admin`.

Сценарий B — напрямую `http://127.0.0.1:8080` с `NEXUS_TRUST_PROXY=false`
(скрипт `tests/trust-proxy-direct.ps1`): неудачный вход с тем же подставным
заголовком, аудит `auth.login.failed ip=127.0.0.1`, `203.0.113.77` отсутствует.
После проверки значение восстановлено в `true`, служба перезапущена.

Доказательства: [`logs/j2-trust-proxy-direct.json`](logs/j2-trust-proxy-direct.json),
[`logs/j2-trust-proxy.log`](logs/j2-trust-proxy.log).

**Правило эксплуатации:** `NEXUS_TRUST_PROXY=true` допустим только при
закрытом снаружи `:8080` и доступе исключительно через Caddy. При прямом
доступе — `false`.

## 5. Публичный домен — BLOCKED

Публичный домен на стенде не проверялся: нет DNS-имени, указывающего на этот
хост, и открытых из интернета 80/443. Let's Encrypt/ACME в таких условиях
непройдёт, поэтому использован внутренний CA. Это не дефект конфигурации.

Инструкция для публичного домена:

1. A/AAAA-запись `nexus.example.com` → внешний IP; 80/443 проброшены.
2. В `Caddyfile` убрать `local_certs`, адрес сайта заменить на
   `nexus.example.com`, `tls internal` убрать.
3. `caddy validate`, `Restart-Service Caddy`; Caddy сам получит сертификат.
4. HSTS включать после проверки, что весь трафик идёт по HTTPS.
5. В `.env`: `NEXUS_PUBLIC_URL=https://nexus.example.com`,
   `NEXUS_COOKIE_SECURE=true`, `NEXUS_TRUST_PROXY=true`.

## 6. Вывод

Все пункты J2 подтверждены на Windows с Caddy for Windows: HTTPS с доверенным
сертификатом, 301 на HTTPS, HSTS, WebSocket Upgrade, Range 206 на 1 ГиБ,
загрузка 1 ГиБ чанками с паузой/продолжением и корректный IP клиента в аудите.
Публичный домен — документированное ограничение стенда.
