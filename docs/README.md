# Документация NEXUS WORKSPACE

| Документ | Содержание |
|---|---|
| [ARCHITECTURE.md](ARCHITECTURE.md) | модули, границы, backend/frontend, jobs, realtime |
| [API.md](API.md) | HTTP API, WebSocket, коды ошибок |
| [AI_WORKSPACE.md](AI_WORKSPACE.md) | AI Chat, Agent, Runtime, протокол, approvals, Skills, ограничения |
| [SECURITY.md](SECURITY.md) | threat model, сессии, CSRF, RBAC, секреты |
| [ADMIN_GUIDE.md](ADMIN_GUIDE.md) | System Management, пользователи, модули, AI Management |
| [USER_GUIDE.md](USER_GUIDE.md) | рабочие пространства, Cloud Storage, AI Workspace |
| [DEPLOYMENT.md](DEPLOYMENT.md) | production, Windows, TLS, backup |
| [INSTALL-WINDOWS.md](INSTALL-WINDOWS.md) | установка NEXUS и службы |
| [DATABASE.md](DATABASE.md) | схема PostgreSQL и миграции |
| [TESTING.md](TESTING.md) | unit/integration/E2E/smoke/load |
| [MANIFEST.md](MANIFEST.md) | контракт манифеста Minecraft |
| [LAUNCHER_CONTRACT.md](LAUNCHER_CONTRACT.md) | API лаунчера |
| [openapi.json](openapi.json) | OpenAPI 3.1 (генерируется) |

AI Runtime: [`../runtime/README.md`](../runtime/README.md).

## Быстрые команды

```powershell
# Backend
cd backend; cargo test -j 2; cargo run -- serve
# Frontend
cd frontend; npm run typecheck; npm run test; npm run build
# Runtime
cd runtime; cargo test --all-targets; cargo build --release
```

## Актуальный статус AI

Фактический статус этапов, тесты и незавершённые пункты —
[`../AI_IMPLEMENTATION_STATUS.md`](../AI_IMPLEMENTATION_STATUS.md).
