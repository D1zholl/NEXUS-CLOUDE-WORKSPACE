# NEXUS 1.1 — Final Release Checklist

**Status of this document:** working checklist for the 1.1 completion effort.
Checked items have a test, command, or code path recorded in this repository.
Unchecked items are open gates; they must not be reported as completed.

**Date:** 2026-11-25
**Base version:** 1.0.0 + uncommitted Agent IDE delta
**Target version:** 1.1.0

## 1. Workspace lifecycle

- [x] Soft delete records a retention deadline and lifecycle state.
- [x] Active tasks block deletion.
- [x] Pending approvals are rejected on deletion.
- [x] Active quota reservations are released on deletion.
- [x] Restore endpoint checks ownership, Runtime availability, data access and
      name conflicts.
- [x] Physical deletion runs in a background worker, never in the HTTP handler.
- [x] Runtime proves `<provisioning_root>/<workspaceId>` ownership and marker.
- [x] Runtime refuses paths outside the provisioning root and reparse points.
- [x] Purge is idempotent and recovers from a trash remainder.
- [x] Failures retry with backoff; ownership failures become `blocked`.
- [x] Snapshots and usage samples are cleaned only after Runtime confirmation.
- [x] Dry-run and metrics are available to administrators.
- [x] Retention is configurable via a system setting (default 30 days).
- [x] Audit events and purge event log are written.
- [x] Unit tests: runtime delete/marker/path cases.
- [x] Integration tests: soft delete, restore, dry-run, successful purge,
      blocked ownership.
- [ ] Physical purge against a real standalone Runtime workspace on a second
      machine (environment gate).

## 2. Quotas

- [x] Central reservation table and advisory-lock transaction.
- [x] Workspace and user quota checked with active reservations.
- [x] All mutating file tools covered.
- [x] `program.run` and MCP use a safe exclusive reservation.
- [x] Reservation is idempotent by operation id.
- [x] Actual usage confirms and shrinks the reservation.
- [x] Known failures release; unknown results remain `uncertain`.
- [x] Expired active reservations are cleaned by a janitor.
- [x] Administrator can release an `uncertain` reservation.
- [x] Concurrent reservation test: 10 parallel writes, 1000-byte quota.
- [x] User-wide quota test across two workspaces.
- [x] Confirm-with-actual-usage test.
- [x] Idempotency and expiry tests.
- [ ] Two real Runtime processes against one workspace in production
      conditions (environment gate; DB reservation is covered by tests).
- [ ] Full end-to-end upload of a file at the quota boundary through Playwright
      (HTTP-level concurrency is covered by backend tests).

## 3. Snapshots

- [x] Existing before/after snapshots, SHA-256 conflict detection and restore
      are preserved.
- [ ] Streaming snapshot for files > 256 KiB.
- [ ] Operation snapshots > 2 MiB.
- [ ] Full recursive directory delete restore.
- [ ] Transaction journal/compensation for multi-file restore.
- [ ] External-change detection independent of mtime.
- [ ] Retention policy and snapshot storage cap.

Phase 3 remains an open gate. The current limitation is documented in
`docs/AI_WORKSPACE.md` and `docs/AI_RUNTIME.md`.

## 4. Processes, terminal, MCP stdio, network isolation

- [ ] `program.start` / `program.poll` / `program.stop` / `program.stdin`.
- [ ] Streaming process events and log retention.
- [ ] Terminal panel and registered ConPTY profiles.
- [ ] MCP stdio transport, admin profiles and lifecycle.
- [ ] Process-level egress isolation (not executable-level firewall rule).
- [ ] Real Windows integration test with administrator rights and a second
      process.

These are Phase 4–7 gates. No mock production path was added.

## 5. Security

- [x] Path traversal / absolute / UNC rejection tests (existing and new).
- [x] Workspace ownership checks on every AI route.
- [x] CSRF, cookie sessions, Argon2id, rate limits (existing tests).
- [x] Secrets redacted from tool arguments and Runtime outcomes.
- [x] MCP SSRF checks and encrypted secrets.
- [x] Purge ownership proof and provisioning-root containment.
- [x] Quota reservation cannot be bypassed via the supported file/agent/MCP
      paths.
- [ ] Final external security audit against a deployed two-machine install.
- [ ] Process-level egress isolation verification.
- [ ] Clean Windows ACL/service-account verification with a non-admin account.

## 6. Testing

- [x] Backend `cargo check --all-targets`.
- [x] Backend `cargo clippy --all-targets --all-features -- -D warnings`.
- [x] Backend `cargo fmt --all -- --check` (after formatting).
- [x] Backend tests (see `docs/reports/FINAL_TEST_REPORT.md`).
- [x] Runtime `cargo check --all-targets`.
- [x] Runtime clippy and fmt.
- [x] Runtime tests.
- [x] Frontend `npm run typecheck`.
- [x] Frontend `npm run lint`.
- [x] Frontend `npm run test`.
- [x] Frontend `npm run build`.
- [ ] Playwright full E2E against a fresh install.
- [ ] Real PolyStrata / WSS / two-machine suite.
- [ ] Standalone clean install and upgrade.

## 7. Standalone / release

- [ ] Rebuild `build-standalone.ps1 -Clean` after the 1.1 changes.
- [ ] Clean Windows 11 install with no Rust/Node/system PostgreSQL.
- [ ] Upgrade from 1.0.0 with preserved data.
- [ ] Backup/restore drill and SHA-256 verification.
- [ ] Final manifest, `SHA256SUMS`, `BUILD-REPORT.txt`, `TEST-REPORT.txt`.
- [ ] Version bump to 1.1.0 and release tag.

Do not create the 1.1.0 tag while any mandatory gate above is unchecked.
