# Deployment Manager

The Deployment Manager stores one build profile per registered website and keeps build-run history.

## Build profiles

A profile contains:
- An allowlisted executable: `node`, `python`, `php`, `cargo`, or `dotnet`
- An argument array (entered one argument per line in the desktop UI)
- A relative output directory that must be created inside the website project

Builds execute with the website project folder as the working directory. WebDynamite invokes the executable directly with separate arguments; it does not concatenate a shell command or invoke PowerShell/cmd.exe. Absolute paths and traversal components are rejected for the output directory, and the resolved output folder must remain inside the project root.

## Running a build

Save a profile, then choose **Run build**. The Core Service captures stdout and stderr, records the exit code and duration, and saves the run in the registry. The UI shows the latest 50 runs for the selected website. Build logs are truncated to 256 KiB.

Build commands execute code from the selected project and should only be run for projects you trust. The executable allowlist reduces shell-injection risk; it is not a sandbox. Builds inherit the Core Service environment and user permissions and can access files and network resources available to that account.

## Runtime process logs

Node.js, PHP, and Python process stdout/stderr are captured through background readers. WebDynamite continues draining both streams after the storage cap is reached, discarding excess output so a chatty application cannot block because its log is full. Each process log is limited to 10 MiB; when the limit is reached, a marker is appended and further output is discarded until the next process start. Starting a process creates a fresh log file.

The website Logs endpoint seeks to the end of the file and returns at most the last 256 KiB (plus a short omission notice when earlier content exists), rather than loading the entire file into memory. The cap is fixed in this version and is not yet configurable.

## Current limitations

- Builds run synchronously in the Core Service request with a configurable timeout (30–1,800 seconds; default 600 seconds) set in **Settings → Hosting defaults**. The timeout applies to new build runs. On Windows, timeout termination targets the build process tree; elsewhere it terminates the direct child process. Build output is redirected to a temporary file and only the last 256 KiB is retained in build history. There is no queue, user cancellation, concurrency limit, or live log streaming yet. Dynamic website startup waits up to 15 seconds for an HTTP response from the assigned loopback port. HTTP 2xx, 3xx and 4xx responses count as ready; 5xx responses, connection failures and early process exits do not.
- Profiles do not store environment variables or secrets.
- Static websites can snapshot build output into immutable managed releases, activate a new release, and roll back to a previous snapshot. Release cleanup is manual; automatic release retention is not implemented.
- Automated release activation is currently static-site-only. Node.js, PHP and Python build profiles can run builds and their processes are checked for an initial HTTP response, but runtime deployment/rollback integration is not implemented.
- Uploading releases to external hosting is not implemented.
- Package-manager wrappers such as `npm.cmd` and shell scripts are not invoked automatically. Use an executable supported on the host and pass arguments separately.
- The Core Service must have the selected executable available on its PATH.


## Release history cleanup

The Git & Releases page can delete an inactive static release snapshot when its files are no longer needed. Deletion permanently removes the snapshot directory from the managed release store and removes its registry entry. The currently active release is protected by the Core Service and cannot be deleted; activate another snapshot first if you need to remove it. Rollback and deletion are scoped to the selected website. Release paths are canonicalized and checked against the managed release directory before filesystem removal.

Automatic pruning runs after a successful release activation. It is not a background timer: changing the retention limit takes effect on the next successful deployment. Manual deletion is also available.


## Runtime crash recovery

Automatic restart is an opt-in global setting, disabled by default. It applies to Node.js, PHP and Python child processes that exit unsuccessfully. Configure the maximum retry count (1–10) and delay (1–300 seconds) in Settings → Hosting. WebDynamite does not restart a manually stopped process or one that exits successfully. Failed launch attempts consume the retry budget; a process that runs for at least five minutes receives a fresh budget after a subsequent crash. Each website's delay and restart attempts run in a separate task, so one site's configured delay or slow startup does not block crash detection for other websites. A manual start/stop or disabling automatic restart while a retry is waiting cancels that pending recovery. This does not replace an external service manager and does not guarantee recovery after the Core Service or Windows exits.


## Build history filters and export

The Deployment Manager build history can be filtered to all, successful, or failed runs. Summary cards show the visible run count, successful and failed counts, and average duration for the filtered set. Export CSV downloads the currently loaded (latest 50 per selected website) filtered rows, including captured logs. CSV cells are quoted and formula-leading values are neutralized before export. Export does not include runs outside the loaded history window.


## Paginated build history

The build history API accepts `limit` (default 50, bounded to 1–100) and `offset` (default 0, bounded to 1,000,000). Results are ordered newest-first with a row identifier tie-breaker for stable ordering. The Deployment Manager initially loads 50 runs and offers **Load older runs** to fetch subsequent pages. Filters, summary cards, and CSV export apply to the runs currently loaded in the UI; refresh resets the list to the newest page.

## Server-side build history filters

The build history endpoint accepts an optional `status` filter (`all`, `success`, or `failed`) and a case-insensitive `search` substring (maximum 120 characters). Search covers the build executable, status and captured output. Filters are applied before pagination, so **Load older runs** continues through the same filtered result set. The desktop UI reloads from the newest page when a filter changes. CSV export includes the filtered records currently loaded in the UI only; it does not silently fetch the entire history.


## Build run details and reproducibility

New build runs store a snapshot of their executable arguments and output directory. The Deployment Manager displays the run ID, exit code, duration, output folder and retained log, and can load the historical configuration into the profile editor. Loading a configuration does not execute code or overwrite the saved profile; saving remains an explicit action. Captured logs remain subject to the existing 256 KiB history limit.
