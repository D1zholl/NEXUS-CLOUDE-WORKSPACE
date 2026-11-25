# NEXUS AI Runtime в Standalone-сборке

`runtime\nexus-ai-runtime.exe` включён в релиз как готовый Windows x64 exe.
Он не требует Rust, Python, Node.js или Visual Studio для запуска.

## Что это

Отдельный процесс, который устанавливает **исходящее** WSS-соединение с
NEXUS (`/api/v1/ai/runtime/connect`), вызывает локальную PolyStrata
(OpenAI-compatible API) и исполняет разрешённые инструменты внутри
изолированных workspace. Входящих портов Runtime не открывает.

## Что требуется дополнительно

1. **PolyStrata** на этом же компьютере с endpoints:
   - `GET /v1/models`
   - `GET /health`
   - `GET /status`
   - `POST /v1/chat/completions`
2. **Enrollment** устройства:
   - войдите в NEXUS как администратор;
   - System Management → AI Management → Runtime → создайте одноразовый
     enrollment code (TTL по умолчанию 15 минут);
   - скопируйте `runtime\runtime.toml.example` в `runtime\runtime.toml` и
     заполните `server.url`, `server.enroll_url`, `storage.allowed_roots`,
   `provider.base_url`;
   - выполните:
     ```
     runtime\nexus-ai-runtime.exe check --config runtime\runtime.toml
     runtime\nexus-ai-runtime.exe enroll --config runtime\runtime.toml --code nxe_...
     runtime\nexus-ai-runtime.exe run --config runtime\runtime.toml
     ```
3. Для постоянной работы можно зарегистрировать службу Windows скриптом
   `runtime\install-service.ps1`, но это **необязательно** и требует
   отдельного решения пользователя. Базовый Standalone работает без службы.

## Состояние без PolyStrata

Если Runtime не настроен или PolyStrata недоступна:

- NEXUS запускается и все остальные модули работают;
- AI-модуль показывает Runtime offline / provider unavailable;
- фиктивных ответов, mock API и демонстрационных данных нет;
- backend не считает AI-модуль обязательным для старта.

## Безопасность

- device token хранится в `runtime\state\device.json` (ACL только для
  пользователя службы), на сервере — SHA-256 hash;
- секреты не пишутся в журналы;
- workspace ограничены `storage.allowed_roots`, проверяются symlink/junction,
  `..`, абсолютные пути и UNC;
- `program.run` — allowlist программ и аргументов, без shell, с approval.

Полная документация исходного Runtime: `runtime\README.md` (в релизе
`docs\AI_RUNTIME_FULL.md`).
