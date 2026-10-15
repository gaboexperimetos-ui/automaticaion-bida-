# Changelog

All notable changes to this project are documented here.
Format: [Keep a Changelog](https://keepachangelog.com/).

## [0.1.0] - 2026-10-09 — Planning Phase

### Added
- Project structure initialized with documentation:
  - `AGENTS.md` — permanent instructions for programming agents
  - `README.md` — project purpose, installation, configuration
  - `docs/REQUIREMENTS.md` — functional and non-functional requirements
  - `docs/ARCHITECTURE.md` — architecture, components, data flows
  - `docs/DECISIONS.md` — architectural decision records (ADRs)
  - `docs/SECURITY.md` — security model, threats, credential handling
  - `docs/TESTING.md` — testing strategy and acceptance criteria
  - `docs/DEPLOYMENT.md` — build, deploy, recovery procedures
  - `docs/ROADMAP.md` — development phases and dependencies
  - `CHANGELOG.md` — this file

### Corrected
- **Critical:** Mission 3 report incorrectly stated MCP files exist on remote
  server. Verified via SSH that `/home/gabo/mikareno-mcp/` does NOT exist on
  `gabo-Inspiron-5720`. The MCP only runs on `asus-tuf`.
- **Systemd CPUQuota semantics:** `CPUQuota=40%` means 40% of ONE CPU core
  (10% of 4 cores), not 40% of all CPUs. Corrected recommendation to
  `CPUQuota=200%` (50% of 4 cores) or removing the limit in favor of `Nice=10`.
- **venv portability:** Python venvs are not portable across distros (glibc
  differences). Must recreate venv on remote with `requirements.txt`, not rsync.
- **systemd --user on remote:** No `systemd --user` session running for `gabo`
  on the remote. Requires `loginctl enable-linger gabo` or system-level service.

### Key findings from missions 0-3
- Local (asus-tuf): MCP active (PID 126524), RSS ~96 MB, port 8080 on 0.0.0.0
- Remote (Ubuntu): No MCP installed. GenOffice MCP active on port 8453. 710 MB free RAM, load avg 9.46/4 cores.
- Tailscale mesh: `100.115.62.97` (local) ↔ `100.92.172.48` (remote)
- Model: `poolside/laguna-s-2.1:free` for development; `cohere/north-mini-code:free` for application
