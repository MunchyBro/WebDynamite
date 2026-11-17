# File Manager

The File Manager operates only inside a registered website's project directory.

## Create folders

Choose a website, navigate to the destination directory, and select **New folder**. The folder is created as a direct child of the current directory. Existing files and folders are never overwritten.

The Core Service validates relative paths, resolves the destination parent inside the canonical website root, and rejects symbolic links, absolute paths, traversal components, missing parents, and duplicate destinations. Folder creation is non-recursive: create parent folders one level at a time.

## Current limitations

- Deleting folders is limited to empty folders.
- Symbolic links are not followed.
- Text editing is limited to UTF-8 files up to 1 MiB.
- File upload/download and archive extraction are not yet implemented.
