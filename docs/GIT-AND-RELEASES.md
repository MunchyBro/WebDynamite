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
