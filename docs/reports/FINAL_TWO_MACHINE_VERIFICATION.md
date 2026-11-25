# NEXUS 1.1 — Two-Machine Verification Report

**Status: NOT EXECUTED in this environment. This is not a pass.**

## 1. Environment available during this session

- One physical Windows host.
- PostgreSQL 17 and 19 services on `127.0.0.1:5432`.
- Rust 1.100.0, Node 26.13.1, npm 11.20.0.
- No second physical computer.
- No real PolyStrata endpoint/model configured.
- No production TLS certificate or reverse proxy under test.
- No administrator-rights process-isolation test.
- Windows service `NexusWorkspace` present but stopped; it was not restarted
  for this run.

## 2. What was verified locally instead

The following are real local checks, not substitutes for the two-machine gate:

- Backend, Runtime and frontend compile; frontend production build succeeds.
- Backend integration tests use a real PostgreSQL database and a real
  in-process WebSocket Runtime test peer.
- Runtime unit/integration tests cover receipts, path isolation, process argument
  policy, MCP HTTP JSON-RPC, workspace delete ownership and markers.
- Lifecycle and quota integration tests use real SQL transactions, concurrent
  reservations and real HTTP handlers.
- Runtime stand tests (`real_runtime_process_over_wss`) remain ignored unless
  `scripts/ai-runtime-stand.ps1` is run.

## 3. Required acceptance procedure (not executed)

1. PC-1: NEXUS backend, PostgreSQL, frontend, storage, HTTPS reverse proxy,
   system management.
2. PC-2: NEXUS AI Runtime, real PolyStrata, selected model, registered
   programs, separate workspace root.
3. Use `wss://` with a valid certificate; no `allow_insecure_ws`.
4. Enroll, rotate token, revoke token, reconnect, heartbeat, backoff.
5. Real model: models list, health, streaming, tool calls, sequential tool
   calls, file creation, multiple-file edit, patch, MCP, cancel, timeout, large
   response, invalid tool call, network break during execution.
6. Network faults: WAN loss, DNS failure, PolyStrata restart, Runtime restart,
   NEXUS unavailability, WSS drop, latency, TLS error, Runtime crash during a
   task.
7. Run `scripts/ai-two-machine-verify.ps1` and preserve its JSON/log output.
8. Record component versions, non-secret configuration, logs, errors, results
   and untested cases in this file.

## 4. Result

No production result is claimed. The release checklist keeps the two-machine
item unchecked. A future session with two physical machines, a real PolyStrata
and a valid TLS certificate must fill this report with actual logs before
1.1.0 can be tagged.
