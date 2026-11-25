# NEXUS 1.1 — Operational Guide: AI Workspace Lifecycle and Quotas

This guide covers the operational behaviour added in the 1.1 completion work.
It is written for administrators of a self-hosted NEXUS instance.

## 1. Workspace lifecycle

### 1.1. States

`ai_workspace_lifecycle.state`:

| State | Meaning | User can restore | Worker acts |
|---|---|---|---|
| `soft_deleted` | User deleted the workspace; files still on Runtime | yes | after `purge_after` |
| `pending` | Administrator requested immediate purge | yes, until claimed | yes |
| `running` | Purge worker holds a lease and is waiting for Runtime | no | lease expires in 10 min |
| `retry` | Temporary purge failure; backoff applied | yes | after new `purge_after` |
| `blocked` | Runtime refused ownership proof; manual action required | no | no automatic retry |
| `purged` | Runtime confirmed physical deletion | no | no |
| `restored` | Workspace was restored | n/a | no |

The retention period is `limits.ai_workspace_retention_days` (1–3650,
default 30). It is a normal system setting and applies without restart.

### 1.2. User flow

1. `DELETE /api/v1/ai/workspaces/{id}`:
   - requires owner or `ai.admin.manage`;
   - rejects deletion while any task is `queued`, `running`, or
     `waiting_approval`;
   - rejects pending approvals for tasks of the workspace;
   - releases active quota reservations;
   - sets `deleted_at`, renames the display name, inserts the lifecycle row.
2. `GET /api/v1/ai/workspaces/deleted` shows restorable items and `purgeAfter`.
3. `POST /api/v1/ai/workspaces/{id}/restore`:
   - requires `ai.workspace.write`;
   - refuses if state is `running`, `blocked`, or `purged`;
   - requires the Runtime to be online;
   - refreshes `workspace.usage` and refuses if Runtime cannot read the data;
   - resolves a name conflict by appending “(восстановлен N)”;
   - clears `deleted_at` and sets state `restored`.

Deletion is not a data-loss operation until the retention period expires.

### 1.3. Physical purge

The worker runs every 60 seconds and claims one due row at a time with
`FOR UPDATE SKIP LOCKED` and a 10-minute lease. It sends:

```json
{
  "type": "workspace.delete",
  "payload": { "workspaceId": "...", "workspaceRoot": "..." }
}
```

Runtime verifies:

1. `workspaceRoot` is an absolute path;
2. canonical path equals `<provisioning_root>/<workspaceId>`;
3. the parent is the canonical `provisioning_root`;
4. the directory is not a symlink/junction/reparse point;
5. `.nexus-workspace.json` (if present) contains the same `workspaceId`;
6. the trash directory is inside `provisioning_root` and is not a reparse point.

Then it renames the directory to `<provisioning_root>/.nexus-trash/<workspaceId>`
and recursively removes it. A repeated command is idempotent
(`alreadyAbsent=true`).

Backend only marks `purged` after `ok=true`. It then deletes
`ai_file_snapshots` and `ai_workspace_usage_samples` for that workspace and
zeroes `usage_bytes/usage_files`. Task history, conversations and audit rows are
kept.

### 1.4. Failure and restart recovery

| Failure | Behaviour |
|---|---|
| NEXUS restarts during purge | `running` row has a lease; after lease expiry it is reclaimed |
| Runtime offline | `retry` with 5/15/60/360-minute backoff |
| Runtime restarts mid-delete | next attempt sees either the original directory or the trash directory and finishes |
| Result of command unknown | row stays `running` until lease expiry; no success is reported |
| Ownership proof fails | `blocked`; no automatic deletion; administrator investigates |
| Partial directory delete | rename already moved the tree out of the workspace namespace; retry removes the trash remainder |

Every attempt is written to `ai_workspace_purge_events` and audit events
`ai.workspace.deleted`, `ai.workspace.restored`, `ai.workspace.purged`,
`ai.workspace.purge_failed`, `ai.workspace.purge_scheduled`.

### 1.5. Administrator endpoints

- `GET /api/v1/ai/admin/workspaces/lifecycle`
- `GET /api/v1/ai/admin/workspaces/purge/dry-run?limit=50`
- `GET /api/v1/ai/admin/workspaces/purge/metrics`
- `POST /api/v1/ai/admin/workspaces/{id}/purge` — queue immediately; the HTTP
  handler still does not delete files.

Dry-run never changes state. To resolve `blocked`, inspect Runtime logs and the
stored `root_path`; if the directory is not a managed provisioning directory,
delete it manually under the Runtime service account or move it out of the
provisioning root, then update the lifecycle row according to your data policy.

## 2. Transactional quotas

### 2.1. Model

`ai_workspaces.usage_*` is a measured snapshot. Active reservations are added
on top:

```
used_workspace = usage_bytes + Σ active/uncertain reservations (workspace)
used_user      = Σ usage_bytes (all non-deleted workspaces) +
                 Σ active/uncertain reservations (user)
```

Reservation is performed inside a transaction holding
`pg_advisory_xact_lock('nexus.ai.quota:<user>')`, so parallel HTTP requests,
agent tool calls, and multiple Runtime connections cannot pass the same check.

### 2.2. Covered operations

`fs.write`, `fs.write_base64`, `fs.copy`, `fs.move`, `fs.delete`, `fs.mkdir`,
`fs.apply_patch`, `program.run`, and all `mcp.*` tools.

| Operation | Estimate |
|---|---|
| write | content size − current size; +1 file if target absent |
| copy | source size − target size; +1 file if target absent |
| move/delete/mkdir | 0 bytes / 0 files (net workspace change) |
| patch | added-line bytes; +1 file per new target |
| program.run / MCP | all remaining workspace/user quota (exclusive write window) |

The estimate is an upper bound. After the operation, Runtime returns actual
usage and the excess reservation is released.

### 2.3. Reservation states

| State | Meaning |
|---|---|
| `active` | operation in flight; counted against quota; expires in 20 minutes |
| `confirmed` | actual usage recorded; no longer counted separately |
| `released` | known failure or administrative release |
| `uncertain` | result unknown (timeout/disconnect/cancel); still counted until resolved |
| `expired` | active lease expired, or uncertain older than 7 days |

For `uncertain`, the safe default is to keep the space reserved. A later
successful usage measurement or an administrator can resolve it:

`POST /api/v1/ai/admin/quotas/reservations/{id}/release`

`GET /api/v1/ai/admin/quotas` includes a `reservations` summary by status.

### 2.4. Monitoring SQL

```sql
-- Active reservations by workspace
select workspace_id, status, count(*), sum(bytes), sum(files)
  from ai_quota_reservations
 where status in ('active', 'uncertain')
 group by 1, 2 order by 4 desc;

-- Rejections in the last day
select event, count(*), sum(requested_bytes), sum(requested_files)
  from ai_quota_events
 where created_at > now() - interval '1 day'
 group by 1;
```

### 2.5. Limits of the guarantee

- Actual external edits are discovered by Runtime's own measurement. The
  backend reservation is only as fresh as the last measured usage, but
  Runtime also enforces the quota against real directory usage before and
  during writes.
- A process that bypasses the NEXUS Runtime and writes directly into the
  workspace on disk is outside the backend reservation path. It is still
  detected by the next Runtime measurement, and `program.run` is killed if it
  exceeds the workspace quota.
- `max_file_bytes` is enforced by Runtime for each file; it is not a
  reservation (a single file may not exceed the limit).
