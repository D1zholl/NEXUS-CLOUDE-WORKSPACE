# NEXUS Standalone Launcher

Native Windows launcher for the portable NEXUS release. Built with Rust and
Win32/GDI (no Electron, no Node.js, no webview).

The binary is copied to the release root as `NEXUS.exe` by
`build-standalone.ps1`.

## CLI

```
NEXUS.exe                    GUI
NEXUS.exe --first-run        GUI (first run)
NEXUS.exe --start            start without GUI
NEXUS.exe --stop             graceful stop
NEXUS.exe --restart          restart
NEXUS.exe --status [--json]  status
NEXUS.exe --diagnostics      report to logs/diagnostics-latest.txt
NEXUS.exe --provision        DB + migrations, no backend
NEXUS.exe --backup           DB dump + storage + config
NEXUS.exe --restore DIR      restore DB/storage (keeps config/secrets.json)
NEXUS.exe --backend ARGS     run backend CLI with the portable environment
```

## Layout and lifecycle

All paths are derived from the directory containing `NEXUS.exe`, never from the
process working directory. The launcher creates `config/`, `data/`, `storage/`,
`logs/`, `cache/`, `backups/`, generates `config/secrets.json` (random DB
password + NEXUS secret, ACL restricted), initializes `data/postgres` with
`initdb`, starts PostgreSQL on 127.0.0.1, applies migrations, optionally creates
the first administrator via the backend CLI, starts `backend\nexus.exe serve`,
waits for `/api/v1/health == ok`, and opens the browser.

`Stop` writes `data/run/shutdown.request`; backend watches `NEXUS_SHUTDOWN_FILE`
and shuts down gracefully. PostgreSQL is stopped with `pg_ctl stop -m fast`.

## PostgreSQL path workaround

Windows `initdb` fails if the executable/share path contains non-ASCII
characters. If the release root is non-ASCII, the launcher creates an NTFS
junction under `%PUBLIC%\NEXUS\pgbin-<hash>` pointing to `database\` and runs
PostgreSQL tools through that ASCII path. Data remains inside the release.

## Verification

`build-standalone.ps1` runs `cargo build --release`; `cargo check` is used
during development. The launcher itself has no unit tests; it is verified by
the end-to-end standalone test report in the release (`TEST-REPORT.txt`).
