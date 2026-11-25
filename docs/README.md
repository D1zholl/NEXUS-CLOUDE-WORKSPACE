# Документация NEXUS WORKSPACE

| Документ | Содержание |
|---|---|
| [ARCHITECTURE.md](ARCHITECTURE.md) | модули, границы, backend/frontend, jobs, realtime |
| [API.md](API.md) | HTTP API, WebSocket, коды ошибок |
| [AI_WORKSPACE.md](AI_WORKSPACE.md) | AI Chat, Agent, Runtime, протокол, MCP, approvals, Skills, ограничения |
| [AI_PRODUCTION_AUDIT.md](AI_PRODUCTION_AUDIT.md) | Phase 9 аудит, исправления, стенд, оставшиеся риски |
| [AI_RUNTIME.md](AI_RUNTIME.md) | Runtime, протокол, tools, `workspace.delete`, квоты |
| [FINAL_COMPLETION_AUDIT.md](FINAL_COMPLETION_AUDIT.md) | финальный аудит 1.1: состояние, проблемы, решения |
| [FINAL_RELEASE_CHECKLIST.md](FINAL_RELEASE_CHECKLIST.md) | обязательные проверки 1.1.0 и открытые gates |
| [FINAL_OPERATIONAL_GUIDE.md](FINAL_OPERATIONAL_GUIDE.md) | lifecycle workspace, retention, purge, резервы квот |
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
