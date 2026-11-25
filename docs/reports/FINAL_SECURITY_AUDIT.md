# NEXUS 1.1 — Final Security Audit

**Date:** 2026-11-25
**Scope:** authentication, authorization, file security, AI tool/Runtime security,
quota lifecycle, Windows deployment assumptions, secret handling.
**Method:** source review, existing test suite, targeted code changes and tests.
This is a code/standalone-host audit; the two-machine deployment audit remains
an external gate.

## 1. Authentication and session

Reviewed:

- server-side cookie sessions, `HttpOnly`, `SameSite`, secure-cookie handling;
- CSRF HMAC token generation and verification for mutating requests;
- Argon2id password hashing;
- session expiry, revoke, login rate limiting and account lockout;
- API tokens and launcher tokens.

Existing tests cover CSRF, rate limiting, lockout, session invalidation after a
password/permission change. No new findings in the 1.1 changes.

## 2. Authorization

Every AI route resolves the user session and then checks:

- module gate (`ai`);
- RBAC permission (`ai.workspace.read/write`, `ai.tools.use`,
  `ai.tasks.cancel`, `ai.actions.approve`, `ai.admin.manage`);
- workspace ownership, except administrators;
- workspace active/deleted state.

New endpoints follow the same model:

| Endpoint | Permission | Ownership |
|---|---|---|
| `GET /ai/workspaces/deleted` | `ai.workspace.read` | owner filter |
| `POST /ai/workspaces/{id}/restore` | `ai.workspace.write` | owner or AI admin |
| `GET /ai/admin/workspaces/lifecycle` | `ai.admin.manage` | admin |
| `GET /ai/admin/workspaces/purge/*` | `ai.admin.manage` | admin |
| `POST /ai/admin/workspaces/{id}/purge` | `ai.admin.manage` | admin |
| `POST /ai/admin/quotas/reservations/{id}/release` | `ai.admin.manage` | admin |

Hiding controls in the frontend is not treated as authorization. The handlers
re-check every call.

## 3. File security

Reviewed and covered by existing tests:

- relative-path normalization, absolute drive paths, UNC, `..`;
- canonical containment inside the workspace root;
- symlink/junction/reparse rejection in Runtime;
- atomic write and optimistic `expectedSha256` conflict handling;
- upload name sanitization;
- MIME sniffing and attachment disposition for non-image content;
- `X-Content-Type-Options: nosniff`;
- oversized upload and body limits;
- concurrent write serialization per workspace.

New lifecycle checks:

- physical deletion accepts only a direct child of `provisioning_root` named
  with the workspace UUID;
- marker `.nexus-workspace.json` is checked; a foreign UUID is rejected;
- symlink/junction and reparse points are rejected for the workspace and trash
  directory;
- the trash directory must canonicalize inside the provisioning root;
- an arbitrary `root_path` from a user cannot be used to delete a directory.

## 4. AI security

- Model-initiated tool calls are still validated by backend JSON Schema,
  workspace allowlist, RBAC, Runtime tool version and Runtime path checks.
- Tool results and file contents are wrapped as untrusted data in the model
  context (`<tool_result untrusted="true">`); they do not grant capabilities.
- MCP endpoints are re-validated on Runtime (scheme, credentials, DNS, private
  address policy); secrets are encrypted and not written to the command outbox.
- Idempotency/receipts prevent replay of side effects; unknown results are not
  retried automatically.
- Quota reservations prevent bypass through concurrent file, agent, patch,
  process and MCP paths. Unknown MCP/process changes reserve the remaining
  quota (fail-safe) and Runtime kills a process that exceeds the workspace
  quota.
- Cancellation is confirmed by Runtime before the task is marked cancelled.
- Prompt injection cannot widen server-side permissions: capabilities come from
  the authenticated user, workspace and Runtime, not from model text.

## 5. Findings

| ID | Severity | Finding | Resolution |
|---|---|---|---|
| SEC-01 | High | Soft-deleted workspace could be physically purged without proving directory ownership | **Fixed.** Runtime verifies canonical `<provisioning_root>/<uuid>`, marker and reparse status; failures become `blocked` |
| SEC-02 | High | Quota check used only last measured usage; concurrent operations could exceed storage/files quota | **Fixed.** Transactional reservations under user advisory lock, confirmed by actual Runtime usage |
| SEC-03 | Medium | A tool result without usage could leave a reservation unconfirmed | **Fixed.** Runtime measures usage after MCP calls; zero reservations are confirmed; non-zero unknown becomes `uncertain` |
| SEC-04 | Medium | Executable-level Windows Firewall rule is not process-level isolation | **Open gate (Phase 7).** Documented; fail-closed when admin rights are absent. Not represented as process isolation |
| SEC-05 | Medium | Multi-file restore can partially apply before an error and has no compensation journal | **Open gate (Phase 3).** Conflict pre-check and per-file audit exist; full transaction journal not implemented |
| SEC-06 | Low | Legacy workspaces without marker files are accepted by exact path convention | Accepted for compatibility; new workspaces always get a marker; documented |
| SEC-07 | Low | Snapshots store file content in PostgreSQL text/base64 | **Open gate (Phase 3).** Size caps documented; no direct model access |

No critical or high findings remain open for the implemented 1.1 features.
SEC-04 and SEC-05 are release gates outside the implemented scope.

## 6. Secret handling

Checked for accidental exposure in logs, OpenAPI, API responses, WebSocket
events, errors and frontend storage:

- API keys and MCP secrets: AES-256-GCM, never returned; outbox stores a
  redacted copy.
- Device tokens: SHA-256 at rest, returned only at enrollment/rotation.
- Enrollment codes: hashed at rest, one-time use, TTL.
- Tool arguments: sensitive keys masked in audit/UI.
- Runtime outcomes: device token and provider key replaced with `<redacted>`;
  absolute workspace roots replaced with `<workspace>`.

A repository scan for hard-coded credentials and private keys was performed;
no production secrets were found in source. `.env` remains excluded by
`.gitignore`; standalone packaging must still be checked by the release gate.

## 7. Windows assumptions

Code and scripts assume:

- Runtime service under a non-privileged service account with access only to
  its state/log/workspace directories;
- provisioning root not shared with Cloud Storage;
- device token file ACL restricted;
- Job Object kill-on-close;
- no inbound ports.

These assumptions were **not re-verified on a clean Windows 11 machine in this
session**. `docs/TWO_MACHINE_CHECK.md` and the service/ACL checklists remain the
acceptance procedure.
