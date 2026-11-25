# NEXUS 1.1 — Release Report

**Date:** 2026-11-25
**Requested release:** 1.1.0
**Actual status:** not tagged, not packaged.

## 1. What is ready

- Backend, Runtime and frontend compile on the current tree.
- Backend clippy, fmt, Runtime clippy/fmt, frontend typecheck/lint/build pass.
- Test suites pass (see `FINAL_TEST_REPORT.md`).
- Workspace lifecycle and transactional quotas are implemented, migrated and
  covered by integration tests.
- OpenAPI regenerated from code: 207 paths, 251 operations.
- Documentation for the new behaviour is present:
  `FINAL_COMPLETION_AUDIT.md`, `FINAL_OPERATIONAL_GUIDE.md`,
  `FINAL_RELEASE_CHECKLIST.md`, `FINAL_SECURITY_AUDIT.md`.

## 2. What blocks 1.1.0

The following mandatory gates were not executed in this environment:

1. Real two-machine NEXUS + Runtime + PolyStrata run.
2. Production WSS/TLS through a reverse proxy with a real certificate.
3. Windows Service under a non-privileged account with final ACL checks.
4. Process-level network isolation and real Windows egress tests.
5. Phase 3 large-file/recursive snapshot restore.
6. Phase 4 long-running process manager and Phase 5 terminal.
7. Phase 6 MCP stdio.
8. Playwright full E2E on a fresh installation.
9. Rebuild of the standalone package from the current tree.
10. Clean Windows 11 install and upgrade from 1.0.0 with data preservation.

## 3. Standalone artifact

The only existing package is
`NEXUS-Standalone-release` (`VERSION=1.0.0`, build time 2026-11-24 21:36).
It predates the lifecycle/quota changes and the Agent IDE delta, so it **must
not** be published as 1.1.0. `BUILD-REPORT.txt` and `SHA256SUMS` in that folder
refer to that older build.

## 4. Decision

The version remains 1.0.0 with an `Unreleased` changelog section. After the
gates in `docs/FINAL_RELEASE_CHECKLIST.md` are closed:

1. bump version to 1.1.0 in the project’s version files;
2. run `build-standalone.ps1 -Clean`;
3. verify the clean install, upgrade and backup/restore;
4. recompute `SHA256SUMS`/manifest;
5. tag the release and attach the final reports.

This is an honest release gate, not a claim of completion.
