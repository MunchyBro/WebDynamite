# Git Integration and Release Management

## Git integration

The Git screen operates on an existing Git working tree inside a registered website folder. It displays the repository root, current branch, HEAD commit, and uncommitted changes.

Supported operations:
- Status inspection
- Fetch remote references (`git fetch --prune --no-tags`)
- Fast-forward-only pull (`git pull --ff-only`)

Git must be installed and available on the Core Service PATH. Git uses the existing Git credential manager; WebDynamite does not accept or persist remote passwords or tokens. Interactive credential prompts are disabled for service operations. Clone, remote editing, branch switching, commit creation and push are not included.

For safety, pull is rejected unless the working tree is clean and the website is stopped. Fetch does not modify the working tree. Git output that could contain remote credentials is not returned to the UI.

## Release snapshots

For static websites, the Release Manager can snapshot the build profile's configured output directory into WebDynamite's managed `releases` data directory. A release records its website, creation time, file count, size and current Git commit when available.

- Snapshot limits: 50,000 files and 2 GiB per release.
- Symbolic links, special files, `.git`, `.env` files, private keys and certificate bundles are rejected.
- The source output must resolve inside the registered project directory.
- The active release is served by the static hosting endpoint; the original project folder remains unchanged.
- Rollback switches the active release pointer. Previous releases remain available for later reactivation.

The current implementation supports release activation and rollback for static websites only. Node.js, PHP and Python processes continue to run from their project folders and do not yet support versioned release switching. Release deletion and retention policies are not included.

## Security and operational notes

Git operations execute fixed Git subcommands without a shell. Git credentials remain in the user's configured Git credential manager. Build output is copied as data; it is not executed by the release manager. The Core Service management API remains loopback-only and all Git/release endpoints require bearer authentication.


## Backup and restore

WebDynamite supports manual full-system backups stored under its local managed backups directory. A backup contains:
- All registered website project folders
- Managed SQLite database contents, copied with SQLite VACUUM INTO and checked with PRAGMA integrity_check
- Website records and runtime settings
- Local and public domain mappings
- Build profiles and build history

Backups are directory snapshots with a manifest and deterministic FNV-1a inventory checksum. The checksum detects accidental corruption; it is not a cryptographic authenticity signature. Each backup is limited to 100,000 files and 10 GiB. Symbolic links and special files are rejected.

Before restoring, the UI verifies the backup and shows a preview of website folders and managed databases that will be overwritten. Restore requires explicit confirmation and all websites must be stopped. WebDynamite automatically creates a fresh pre-restore safety backup first. Website folders are staged beside their destination before activation.

Backups currently remain on the same PC. For disaster recovery, copy the managed backup directory to a separate disk or off-device location. Scheduled backups, retention policies, user-selected backup locations, release snapshot payloads and cryptographic signatures are not included in this phase.
