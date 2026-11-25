NEXUS WORKSPACE 1.0.0 — portable standalone release for Windows 11 x64.

Start here: README-FIRST-RUN.txt

Quick start:
  1. Keep the whole folder together.
  2. Run NEXUS.exe or Start.bat.
  3. Create the first administrator in the console window (login: admin).
  4. NEXUS opens at http://127.0.0.1:8080.

No Rust, Node.js, npm, Visual Studio, Git, IDE or system PostgreSQL is
required. The release contains a production backend, production frontend,
portable PostgreSQL 17, NEXUS AI Runtime and a native launcher.

Control scripts:
  Start.bat, Stop.bat, Restart.bat, Status.bat, First-Run.bat,
  Diagnostics.bat, Backup.bat, Restore.bat, Uninstall.bat

Data is stored inside this folder: config\, data\, storage\, logs\,
backups\, cache\. The program files are backend\, frontend\, database\,
runtime\, docs\, LICENSES\, tools\ and NEXUS.exe.

AI Runtime requires external PolyStrata and manual enrollment; see
docs\AI_RUNTIME.md. All other modules work without it.
