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

## Current limitations

- Builds run synchronously in the Core Service request; there is no queue, cancellation, concurrency limit, build timeout, or live log streaming yet. Dynamic website startup waits up to 15 seconds for an HTTP response from the assigned loopback port. HTTP 2xx, 3xx and 4xx responses count as ready; 5xx responses, connection failures and early process exits do not.
- Profiles do not store environment variables or secrets.
- Static websites can snapshot build output into immutable managed releases, activate a new release, and roll back to a previous snapshot. Release cleanup is manual; automatic release retention is not implemented.
- Automated release activation is currently static-site-only. Node.js, PHP and Python build profiles can run builds and their processes are checked for an initial HTTP response, but runtime deployment/rollback integration is not implemented.
- Uploading releases to external hosting is not implemented.
- Package-manager wrappers such as `npm.cmd` and shell scripts are not invoked automatically. Use an executable supported on the host and pass arguments separately.
- The Core Service must have the selected executable available on its PATH.


## Release history cleanup

The Git & Releases page can delete an inactive static release snapshot when its files are no longer needed. Deletion permanently removes the snapshot directory from the managed release store and removes its registry entry. The currently active release is protected by the Core Service and cannot be deleted; activate another snapshot first if you need to remove it. Rollback and deletion are scoped to the selected website. Release paths are canonicalized and checked against the managed release directory before filesystem removal.

Automatic pruning runs after a successful release activation. It is not a background timer: changing the retention limit takes effect on the next successful deployment. Manual deletion is also available.
