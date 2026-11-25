# NEXUS WORKSPACE 1.1 — Final Completion Audit

**Date:** 2026-11-25
**Branch:** `main`
**Baseline commit:** `0101a63` (`feat(ai): Phase 9 production hardening`)
**Working tree at audit:** uncommitted AI Agent IDE delta (`0021`, `0022`, `workspace_files`, agent UI).
**Backup:** tag `backup-pre-1.1-worktree` (full tree commit `34554e8`) + tag
`backup-pre-1.1-audit` (`0101a63`).

This document records the state **as found in code**, not claims from earlier
plans. A feature is "working" only where an implementation path and a test or
reproducible check exist.

---

## 1. Baseline verification

| Check | Command | Result |
|---|---|---|
| Git cleanliness | `git status --short` | 184 changed/untracked paths; preserved in backup commit before any edit |
| Backend type check | `cargo check --all-targets` (`backend/`) | PASS |
| Runtime type check | `cargo check --all-targets` (`runtime/`) | PASS |
| Frontend types | `npm run typecheck` (`frontend/`) | PASS |
| Backend tests | `cargo test -j 2 --no-fail-fast` | see `docs/reports/FINAL_TEST_REPORT.md` |
| Runtime tests | `cargo test --all-targets` | see `docs/reports/FINAL_TEST_REPORT.md` |
| Frontend tests | `npm run test` | see `docs/reports/FINAL_TEST_REPORT.md` |
| PostgreSQL | no system `psql` in `PATH` | tests that need a database use the project test helper/portable PostgreSQL |

Toolchain observed: Rust 1.100.0, Node 26.13.1, npm 11.20.0.

---

## 2. Component audit

| Component | State | Found problem | Resolution | Verification |
|---|---|---|---|---|
| AI Chat (`/workspace/ai`) | Implemented | Streaming/reconnect paths exist; production provider not exercised here | Keep; regression tests | Backend AI tests, frontend tests |
| Agent IDE (`/workspace/ai/agent`) | Implemented (uncommitted delta) | New code was not in a commit; standalone built before it | Commit delta; rebuild standalone | Typecheck/build + Playwright |
| Workspace Explorer / editor | Implemented via Runtime tools | All file operations go through `tool.execute`; no server-side disk access | Keep single executor | `workspace_files` tests, E2E |
| Workspace delete | **Completed in 1.1 work** | `deleted_at` set, physical files and snapshots remain forever; no queue, no restore endpoint, no retention setting | Implemented: purge queue, restore, retention setting, Runtime-verified physical delete | `ai_tests` lifecycle tests; runtime delete tests |
| Workspace restore | **Completed in 1.1 work** | No endpoint or state transition | Implemented: owner/admin, Runtime/data check, name conflict handling | `workspace_soft_delete_is_restorable...` |
| Workspace provisioning | Implemented | Creates `<provisioning_root>/<uuid>` without an ownership marker | Implemented: marker + delete ownership proof | `runtime::workspace_lifecycle_tests` |
| Quota: task slots | Implemented | `reserve_task_slot` uses advisory lock | Keep | Existing tests |
| Quota: storage/files | **Completed in 1.1 work** | Checks use last measured `ai_workspaces.usage_*`; no reservation; concurrent writes and multiple paths can race; copy/move/delete are not centrally covered | Implemented: `ai_quota_reservations` + transactional reserve/confirm/release for every mutating tool | Concurrent, idempotency, expiry, user-wide tests |
| `max_file_bytes` | Implemented | Enforced in Runtime and user API | Keep | Existing tests |
| `max_parallel_tasks` | Implemented | Per user and per workspace | Keep | Existing tests |
| Snapshots | **Limited** | 256 KiB/file, 2 MiB/operation, text/base64 in PostgreSQL; recursive directory delete and big files are marked `truncated`; no streaming/version manifests | Phase 3 (not completed in this session) — documented gate | Snapshot tests; limitation documented |
| Snapshot restore | Implemented for supported files | Pre-check + SHA conflict detection; partial-failure rollback journal absent | Phase 3 gate | Restore tests |
| External file changes | Partial | `expectedSha256` on writes and restore; no watcher/continuous hash index | Phase 3 gate | Conflict tests |
| `program.run` | Implemented | Synchronous, timeout-bound, allowlist, Job Object, output cap | Keep | Runtime tests |
| `program.start/poll/stop/stdin` | **Missing** | No long-running process registry, no per-process streaming, no stop confirmation beyond task cancel | Phase 4 gate | — |
| Interactive terminal | **Missing** | No ConPTY/terminal profiles; model must not get a shell | Phase 5 gate | — |
| MCP Streamable HTTP | Implemented | SSRF checks, discovery, tool call, assignment/policies | Keep | Runtime MCP test |
| MCP stdio | **Missing** | Runtime only has HTTP transport; no admin stdio profile | Phase 6 gate | — |
| Network policy | **Executable-level** | Windows Firewall rule blocks all processes of an executable, not one process; needs admin; startup leaves no explicit stale-rule recovery | Phase 7 gate; fail-closed behaviour verified | Runtime unit tests; real Windows test required |
| Cancel | Implemented | `task.cancel` waits for active operations; `confirmed=false` surfaced | Keep | AI tests |
| Receipts / idempotency | Implemented | Rotated receipts keep a pruned index; unknown results are not retried | Keep | Runtime receipt tests |
| Runtime recovery | Implemented | `/hello` reconciles tasks/commands; pending replies fail as unknown | Keep | Standalone runtime test |
| Runtime WSS/TLS | Implemented in code | No real certificate/production proxy run in this environment | External gate (Phase 8) | `docs/TWO_MACHINE_CHECK.md` + script |
| RBAC | Implemented | Permissions catalog has `ai.workspace.read/write`, `ai.tools.use`, `ai.tasks.cancel`, `ai.admin.manage`; handlers call `require` | Keep; extend new endpoints | Permission tests |
| CSRF / sessions / Argon2id | Implemented | Existing platform tests | Keep | Security tests |
| Audit | Implemented | File/tool/workspace/admin events | Extend with purge/restore/reservation events | Audit tests |
| OpenAPI | Partial | New agent workspace routes partly documented; new lifecycle/quota routes absent | Phase 12 update | OpenAPI diff |
| Background jobs | Implemented | Scheduler + retry/pause; no AI workspace purge job | Phase 1 adds purge worker | Job tests |
| Windows service | Implemented | Service install scripts exist; real service run not repeated here | External gate | `docs/reports/*service*` |
| Standalone build | Implemented (artifact 2026-11-24 21:37) | Built before the latest uncommitted delta; clean-Windows install not re-verified | Rebuild + gate | `BUILD-REPORT.txt`, `SHA256SUMS` |
| Documentation | Large but partly stale | `AI_RUNTIME.md` explicitly says long-running processes/terminal are absent; `COMPLETE.md`/`CHANGELOG` predate this work | Phase 12 update; keep open limitations | Doc review |
| TODO/FIXME | None in Rust sources | No hidden stubs found by `TODO/FIXME/unimplemented!/todo!` scan | — | `rg` scan |
| Two physical computers | **Not verified** | No second machine or real PolyStrata in this environment; prior reports are plans/single-host stands | Phase 8 gate; do not claim pass | `scripts/ai-two-machine-verify.ps1` |

---

## 3. Security findings from code review

| ID | Severity | Finding | Status |
|---|---|---|---|
| A-01 | High | A soft-deleted workspace can be purged without an ownership proof from Runtime; a supplied path alone must never be trusted | **Fixed:** UUID + marker + canonical provisioning-root checks; blocked on mismatch |
| A-02 | High | Storage quota is not reserved transactionally; two concurrent writes can both pass the last-measured check | **Fixed:** transactional reservations under user advisory lock |
| A-03 | Medium | Snapshot rollback of a multi-file restore executes sequentially and has no compensation journal; a mid-way Runtime error can leave a partial restore without per-file report | Phase 3 gate |
| A-04 | Medium | Network policy is executable-scoped, so it is not process-level isolation | Phase 7 gate; status text must not overclaim |
| A-05 | Medium | `raw_file` serves inline images only, but SVG/HTML are attachment + `nosniff`; verify no browser rendering path bypasses this | Re-checked; no bypass found |
| A-06 | Low | Admin can read a soft-deleted workspace's files (`accessible` allows admin) | Intentional for recovery/diagnostics; purge is admin-visible and audited |

No hard-coded credentials were found in Rust/TS sources. Secrets are redacted in
tool arguments and Runtime outcomes.

---

## 4. Environment gates (cannot be closed from this workspace)

These are **not** marked as passed. They require physical or privileged
resources and have reproducible procedures in `docs/TWO_MACHINE_CHECK.md` and
`scripts/ai-two-machine-verify.ps1`.

1. Two physical PCs (NEXUS server + Runtime/PolyStrata).
2. Real PolyStrata model, streaming and tool calling.
3. Production WSS/TLS with a real certificate and reverse proxy.
4. Clean Windows 11 install of the standalone build.
5. Upgrade from 1.0.0 on a populated database.
6. Windows service running under a non-privileged account.
7. Real process-level egress isolation test with administrator rights.
8. ConPTY terminal and MCP stdio real-process acceptance.
9. Large-file/recursive snapshot restore under real disk pressure.

Each gate has an explicit acceptance record requirement; absence of the record
means "not verified", never "passed".

---

## 5. Phase order used for the remaining work

1. Phase 0 — this audit (committed separately).
2. Phase 1 — workspace lifecycle.
3. Phase 2 — transactional quota reservations.
4. Phase 3+ — implemented only where a safe, complete change fits; otherwise
   recorded as an open gate with a concrete design. No mock production paths.
5. Documentation and test reports are updated from actual runs.

The final acceptance checklist will contain unticked items where the gate was
not executed.
