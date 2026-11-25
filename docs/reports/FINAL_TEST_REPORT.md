# NEXUS 1.1 — Final Test Report

**Date:** 2026-11-25
**Host:** single Windows 11 machine
**Database:** PostgreSQL on `127.0.0.1:5432` (`nexus` for upgrade check,
`nexus_test` for integration tests)

Commands were run on the final code of this session. Results below are copied
from the actual runs, not expected values.

## 1. Backend

| Command | Result |
|---|---|
| `cargo fmt --all -- --check` | PASS |
| `cargo check --all-targets` | PASS |
| `cargo clippy --all-targets --all-features -- -D warnings` | PASS |
| `cargo test -j 2 --no-fail-fast` | see section 1.1 |
| `cargo test --test ai_tests` | 28 passed, 0 failed |
| `cargo test --test migrations_tests` | 2 passed, 0 failed |
| `cargo test --test openapi_tests` | 5 passed, 0 failed |

### 1.1. Full backend suite

`cargo test -j 2 --no-fail-fast` log: `build/phase2-backend-tests.log`.

**443 passed, 0 failed, 1 ignored** across 33 test binaries (including 101
library unit tests, 4 service unit tests, 28 AI integration tests and the
existing platform suites). Ignored: the optional real Runtime stand test,
explained below.

One test is intentionally ignored:

- `tests/ai_runtime_stand.rs::real_runtime_process_over_wss` — requires a
  separately built Runtime and `scripts/ai-runtime-stand.ps1`. This is a real
  integration gate, not a mock.

### 1.2. New backend tests

In `backend/tests/ai_tests.rs`:

- `workspace_soft_delete_is_restorable_and_dry_run_has_no_side_effects`
- `purge_worker_confirms_runtime_deletion_before_cleaning_records`
- `purge_worker_blocks_on_unverified_ownership_and_keeps_data`
- `concurrent_quota_reservations_cannot_exceed_workspace_limit` (10 parallel
  reservations, 1000-byte workspace quota, exactly 5 granted)
- `quota_reservation_is_idempotent_and_expires_after_restart_window`
- `quota_confirm_uses_actual_usage_and_releases_excess`
- `quota_is_shared_across_all_workspaces_of_one_user`

Existing AI tests were re-run without changes to their assertions; 28/28 pass.

## 2. Runtime

| Command | Result |
|---|---|
| `cargo fmt --all -- --check` | PASS |
| `cargo check --all-targets` | PASS |
| `cargo clippy --all-targets --all-features -- -D warnings` | PASS |
| `cargo test --all-targets` | **41 passed, 0 failed, 1 ignored** |

Ignored: `provider::tests::real_polystrata_smoke` — requires a running real
PolyStrata and model. This is an external acceptance gate.

New runtime tests:

- marker roundtrip;
- delete only a managed workspace and idempotent retry;
- reject a path outside the provisioning root;
- reject a foreign marker;
- accept a legacy workspace without a marker only by exact path convention.

## 3. Frontend

| Command | Result |
|---|---|
| `npm run typecheck` | PASS |
| `npm run lint` | PASS (`--max-warnings=0`) |
| `npm run test` | **126 passed, 0 failed** (22 files) |
| `npm run build` | PASS (Vite 7, production bundle) |

The build warns that some chunks exceed 1200 kB. This is a size warning, not a
failure; no runtime error was observed.

## 4. Database

| Check | Result |
|---|---|
| Empty database migration test | PASS: all 24 files applied, `_sqlx_migrations` matches |
| Existing database upgrade (`nexus migrate` on the dev DB) | PASS: 24 migrations, `ai_workspace_lifecycle` and `ai_quota_reservations` exist |
| Repeated migrate | Covered by `migrations_apply_to_empty_database` and idempotent SQL |
| Rollback | Not applicable: 0023/0024 are additive; no destructive rollback is provided |

## 5. OpenAPI

- Regenerated from `nexus::api::openapi::specification` into
  `docs/openapi.json`.
- `node scripts/gen-api-doc.mjs`: **207 paths, 251 operations**, 207 `.route(`
  registrations.
- `openapi_tests`: 5/5 pass (coverage, unique operations, path parameters,
  OpenAPI 3.1 validity, path count).

## 6. Not executed

These are not reported as passes:

- Playwright E2E against a fresh installation (existing suite is present but
  was not run in this session).
- Real PolyStrata model test.
- Two-machine WSS/TLS test.
- Windows process-level egress test.
- Clean Windows 11 install and upgrade from a packaged 1.0.0 build.
- Real large-file/recursive snapshot restore.
