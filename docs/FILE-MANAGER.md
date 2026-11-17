# File Manager

The File Manager operates only inside a registered website's project directory.

## Upload and download files

Choose a website and destination folder, then select **Upload files** to select multiple files. Files are uploaded sequentially. Use **Upload files** for the current folder or **Upload folder** to preserve a selected directory tree. Each file may be up to 16 MiB; one batch accepts at most 100 files and 256 MiB total. Existing files are never overwritten; conflicts are reported per file and do not stop the rest of the batch. To retrieve a regular file, use its download action; downloads are limited to 32 MiB. Binary files are supported for transfer even though the built-in editor remains text-only.

Uploads use the authenticated Core Service API, validate relative paths, and create files exclusively inside the selected website root. Symbolic links are not followed and existing destinations return a conflict.

## Create folders

Choose a website, navigate to the destination directory, and select **New folder**. The folder is created as a direct child of the current directory. Existing files and folders are never overwritten.

The Core Service validates relative paths, resolves the destination parent inside the canonical website root, and rejects symbolic links, absolute paths, traversal components, missing parents, and duplicate destinations. Folder creation is non-recursive: create parent folders one level at a time.

## Current limitations

- Deleting folders is limited to empty folders.
- Symbolic links are not followed.
- Text editing is limited to UTF-8 files up to 1 MiB.
- Uploads are limited to 16 MiB per file, 100 files and 256 MiB per batch; downloads are limited to 32 MiB per file. Bulk download and archive extraction are not yet implemented. Folder upload preserves relative subdirectories but is limited to the same 100-file / 256 MiB batch caps.
