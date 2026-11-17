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

- Builds run synchronously in the Core Service request; there is no queue, cancellation, concurrency limit, timeout, or live log streaming yet.
- Profiles do not store environment variables or secrets.
- This phase verifies the configured output folder but does not copy artifacts to a separate release directory, create immutable deployments, roll back releases, or upload to external hosting.
- Package-manager wrappers such as `npm.cmd` and shell scripts are not invoked automatically. Use an executable supported on the host and pass arguments separately.
- The Core Service must have the selected executable available on its PATH.
