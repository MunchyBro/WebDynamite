# Settings, Diagnostics and Recovery

## Persistent settings

Settings are stored in the internal SQLite database (schema migration 10) and are exposed only through authenticated Core Service endpoints.

Editable preferences:
- Default website directory: optional existing absolute directory, prefilled in the Add Website workflow.
- PHP executable: executable name or absolute path used when starting PHP sites.
- Python executable: executable name or absolute path used when starting Python static-file sites.
- Log retention: 1–365 days; older files in the managed log directory are removed when this preference is saved.
- Monitoring refresh interval: 2–60 seconds in the desktop dashboard.
- Default local domain suffix: `.localhost` or `.test`, used as the domain form hint.

The settings API accepts only these keys and validates types and ranges. It does not accept arbitrary configuration keys or commands.

## Monitoring history and alerts

The Monitoring page keeps up to 60 host metric samples in desktop-app memory for the current session. Samples are collected on the configured monitoring interval while Overview or Monitoring is visible. CPU, memory and aggregate disk usage trends are shown as percentages; the history can be exported as CSV for later analysis. The history is intentionally session-only and is cleared when the desktop app restarts.

The page also highlights current CPU, memory or disk usage at or above 85%. These are local advisory indicators, not background notifications or automated remediation. Network counters remain cumulative operating-system counters rather than transfer-rate samples.

## Diagnostics

The authenticated diagnostics report checks:
- Core Service availability, uptime and loopback management bind.
- SQLite `PRAGMA integrity_check` and website / managed-database counts.
- Node.js, npm, configured PHP and Python executables, Git and Caddy availability/version. Runtime version probes run concurrently, each with a five-second timeout, a 16 KiB output cap and a 512-character displayed version line. A hung or broken executable is reported as a diagnostic failure instead of holding the diagnostics request indefinitely.
- Existence and write access of managed logs, releases, backups and configuration directories.
- Number of currently supervised child processes.
- Security invariants: management API remains loopback-only and public exposure remains opt-in.

Diagnostics do not transmit reports to a remote service. Executable version checks run local programs and show only the first output line.

## Recovery

At Core Service startup, interrupted `starting` and `stopping` transitions are reset to `stopped`. Persisted `running` records are deliberately left unchanged: a service restart alone cannot prove whether an untracked runtime child survived, so startup does not guess or terminate arbitrary processes.

The manual reconciliation action inspects persisted website states against the Core Service's in-memory process registry. It resets stale `starting`, `stopping` or non-static `running` states to `stopped` when there is no tracked child process. A static website in `running` state is left unchanged because static hosting is served by the Core Service itself.

The action does not kill arbitrary processes, alter firewall rules, modify DNS, rewrite Caddy configuration, or delete website data. Caddy can be independently validated, started, stopped and reloaded from the existing proxy controls.

## Fixed safety defaults and limitations

- The management API remains bound to `127.0.0.1:47821`; it cannot be changed through the settings UI.
- Public exposure remains explicit and opt-in.
- Core Service startup mode, Windows service installation/autostart, Caddy executable configuration, custom backup storage location and automatic update channels are not editable in this phase.
- Runtime executable paths apply to PHP and Python. Node.js websites continue to launch through the platform's npm command resolution.
- Log retention cleanup is performed when the setting is saved; there is no scheduled background cleanup task yet.
