# File Manager

The File Manager operates only inside a registered website's project directory.

## Upload and download files

Choose a website and destination folder, then select **Upload**. Uploads accept individual files up to 16 MiB and never overwrite an existing file. To retrieve a regular file, use its download action; downloads are limited to 32 MiB. Binary files are supported for transfer even though the built-in editor remains text-only.

Uploads use the authenticated Core Service API, validate relative paths, and create files exclusively inside the selected website root. Symbolic links are not followed and existing destinations return a conflict.

## Create folders

Choose a website, navigate to the destination directory, and select **New folder**. The folder is created as a direct child of the current directory. Existing files and folders are never overwritten.

The Core Service validates relative paths, resolves the destination parent inside the canonical website root, and rejects symbolic links, absolute paths, traversal components, missing parents, and duplicate destinations. Folder creation is non-recursive: create parent folders one level at a time.

## Current limitations

- Deleting folders is limited to empty folders.
- Symbolic links are not followed.
- Text editing is limited to UTF-8 files up to 1 MiB.
- Uploads are limited to 16 MiB per file and downloads to 32 MiB; bulk transfer and archive extraction are not yet implemented.
