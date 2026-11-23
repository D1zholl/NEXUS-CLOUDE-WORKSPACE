# Security и Filesystem Layer

Реализация находится в `crates/nexus-platform/src/security/`.

Rust-ядро — единственный владелец операций с файловой системой. Frontend не
получает файловых, shell- или HTTP-разрешений Tauri; он вызывает только
типизированные команды.

## 1. Файловая система

### `SafeFs`

`SafeFs` — capability-обёртка над одним каноническим корнем. Все методы
принимают только `SafeRelativePath`, который невозможно построить из
абсолютного пути, пути с `..` или небезопасного имени.

Основные операции:

| Метод | Назначение |
|---|---|
| `ensure_dir` | пошаговое создание каталогов с проверкой каждого компонента |
| `write_atomic` | временный файл в том же каталоге + flush + атомарный rename |
| `write_atomic_from_reader` | то же с потоковым лимитом размера |
| `read` | чтение с лимитом и запретом reparse point |
| `remove_file` / `remove_dir_all` | удаление без перехода по ссылкам |
| `rename` | безопасное перемещение, fallback для разных томов |
| `create_staging` | уникальный staging-каталог с автоочисткой |
| `verify` / `hash` | SHA-256 и проверка размера |
| `resolve` / `resolve_existing` | проверка реального пути и принадлежности корню |

### Защита от Path Traversal

Проверяется не только строка:

1. абсолютные пути, drive-relative (`C:evil`), UNC, `..`, пустые компоненты
   отклоняются до I/O;
2. каждый существующий компонент проверяется через `symlink_metadata`;
3. канонический путь проверяется на принадлежность корню;
4. запрещены NTFS ADS (`file.txt:stream`), зарезервированные имена Windows
   (`CON`, `NUL`, `COM1`, ...), завершающие точки/пробелы;
5. для записи/переименования удерживаются handles всех родительских каталогов
   без `FILE_SHARE_DELETE`. На Windows это не даёт переименовать или удалить
   каталог между проверкой и записью.

### Символические ссылки, junctions и reparse points

`is_reparse_point` учитывает `FILE_ATTRIBUTE_REPARSE_POINT`, поэтому
обнаруживаются не только symlink, но и junction/mount point.

- промежуточный компонент-ссылка → `REPARSE_POINT_REJECTED`;
- запись в ссылку или через неё запрещена;
- `remove_dir_all` удаляет саму ссылку/junction и никогда не обходит её;
- ZIP-элементы-ссылки отклоняются целиком.

### Атомарная запись и подмена файлов

Запись выполняется в `NamedTempFile` внутри целевого каталога, затем
`sync_all` и rename. Для установки:

1. файл скачивается/распаковывается в уникальный staging-каталог;
2. SHA-256 и размер проверяются до перемещения;
3. файл атомарно перемещается в целевой каталог;
4. SHA-256 проверяется повторно уже по конечному пути;
5. при несовпадении файл удаляется и возвращается `CHECKSUM_MISMATCH`.

`hash_file` проверяет размер до и после чтения, поэтому рост/усечение файла во
время хеширования обнаруживается.

## 2. ZIP

`security::fs::archive`:

- `inspect_zip` — проверка без распаковки;
- `extract_zip_to_new_dir` — транзакционная установка в новый каталог;
- `extract_zip_into` — слияние с существующим каталогом (каждый файл
  устанавливается атомарно).

Проверки:

- Zip Slip (`../`, `..\`, `/abs`, `C:\`, UNC);
- symlink-элементы и encrypted-элементы;
- количество элементов;
- размер одного элемента;
- суммарный распакованный размер;
- коэффициент сжатия (Zip Bomb);
- дубликаты путей и конфликты «файл/каталог» без учёта регистра;
- фактический размер и CRC-32 каждого элемента;
- отсутствие reparse points в готовом дереве.

Лимиты по умолчанию: 2 ГиБ архив, 100 000 элементов, 512 МиБ на элемент,
4 ГиБ суммарно, коэффициент сжатия 200. `ArchiveLimits::strict()` — более
жёсткий профиль.

## 3. SHA-256 и размеры

`Sha256Digest` парсит строго 64 hex-символа, сравнение выполняется в
постоянном времени. `ExpectedFile` позволяет проверять хеш, размер или оба
значения. Все потоковые операции принимают `max_size` и обрываются при его
превышении, не полагаясь на заголовок `Content-Length`.

## 4. Секреты

`security::secrets`:

- `SecretRef` — типизированная ссылка (`account`, `source`, `generic`), а не
  сам секрет;
- `SecretValue` — redacted `Debug`/`Display`, zeroize при уничтожении;
- `SecretStore` — типизированный trait;
- `KeyringSecretStore` — Windows Credential Manager;
- `DpapiSecretStore` — DPAPI-шифрование файлов, имена файлов — SHA-256 ссылки.

В SQLite допускается только `secret_ref`. При недоступности защищённого
хранилища операции завершаются ошибкой `SECRET_STORE_UNAVAILABLE`; plaintext
fallback отсутствует.

## 5. URL и сеть

`security::net::url`:

- по умолчанию только HTTPS и порт 443;
- отклоняются `file:`, `javascript:`, userinfo, fragments и опасные схемы;
- проверяются loopback, private, CGNAT, link-local, multicast, broadcast,
  documentation и reserved диапазоны;
- link-local (`169.254.0.0/16`, `fe80::/10`) блокируется всегда, даже при
  разрешённых локальных адресах — это защищает cloud metadata endpoints;
- DNS-имена разрешаются до соединения, проверяются **все** адреса, затем
  адреса закрепляются в HTTP-клиенте (защита от DNS rebinding);
- `NexusSourceUrl` нормализует пользовательский адрес NEXUS, запрещает query и
  не нормализованные сегменты.

`security::net::fetch`:

- автоматические редиректы отключены; каждый hop проверяется заново;
- запрещён downgrade HTTPS → HTTP;
- ограничено число редиректов;
- есть connect/request timeout;
- тело ответа читается потоково и обрывается по лимиту;
- `download_to` скачивает в staging, проверяет ожидаемый SHA-256/размер и
  атомарно устанавливает файл.

### Исключения для self-hosted NEXUS

Исключения не включаются автоматически. В `config.toml`:

```toml
[security]
verify_tls = true
# Разрешить private/LAN и loopback адреса для NEXUS Workspace
allow_local_nexus_sources = true
# Разрешить HTTP для локального self-hosted сервера (осознанное решение)
allow_insecure_http_nexus_sources = true
```

`verify_tls = false` запрещён валидацией конфигурации. Даже с включёнными
исключениями link-local/metadata адреса остаются заблокированными.

## 6. Tauri

- capability `main-capability` содержит только `core:default`;
- нет плагинов `shell`, `fs`, `http`, `process`, `updater`;
- `freezePrototype: true`;
- строгий CSP: `default-src 'self'`, `object-src 'none'`,
  `frame-ancestors 'none'`, `base-uri 'self'`, `form-action 'self'`;
- `connect-src` ограничен IPC;
- аргументы команд строго типизированы, `SettingsPatch` использует
  `deny_unknown_fields`, пути проходят `validate_directory`;
- секреты и `SafeFs` живут только в Rust-состоянии и не передаются в WebView.

## 7. Остаточные риски

- Процесс, работающий под той же учётной записью Windows и имеющий право
  записи в каталог, в принципе может атаковать файлы, которые он уже может
  изменять. Для установки это компенсируется повторной проверкой SHA-256 после
  атомарного перемещения.
- `safe_move` для каталогов между разными томами использует копирование и
  проверку отсутствия ссылок; прерывание может оставить временный каталог,
  который удаляется при ошибке.
- Полноценная защита от атакующего с правами администратора/отладчика не
  входит в модель угроз пользовательского лаунчера.

## 8. Тесты

- unit-тесты модулей `security::fs::*`, `security::net::*`,
  `security::secrets`;
- `crates/nexus-platform/tests/security_filesystem.rs` — реальные junction на
  Windows, path traversal, подмена файла;
- `src-tauri/tests/security_config.rs` — минимальные capabilities, CSP,
  отсутствие shell/fs/http доступа и проверка аргументов через
  `deny_unknown_fields`.

Запуск:

```powershell
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```
