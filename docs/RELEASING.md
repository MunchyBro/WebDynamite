# Windows releases

WebDynamite builds a Windows NSIS installer through the **Windows Installer** GitHub Actions workflow.

## Build an installer manually

1. Open **Actions → Windows Installer** in the GitHub repository.
2. Select **Run workflow** on the default branch.
3. Wait for the Windows build to finish.
4. Download the `WebDynamite-Windows-Installer` artifact from the completed run.

The workflow builds the Core Service first so Tauri can bundle `webdynamite-core.exe` into the desktop application.

## Publish a versioned release

Push a version tag such as `v0.1.0`. The workflow builds the installer, uploads it as a workflow artifact, and attaches the installer to a GitHub Release with generated release notes.

Keep the tag version aligned with the application versions in `package.json`, `src-tauri/Cargo.toml`, and `src-tauri/tauri.conf.json`.

## Signing status

Installers produced by this workflow are **unsigned** unless code-signing is configured separately. Do not treat an unsigned installer as a production-trustworthy distribution. Before public distribution, configure an appropriate Windows code-signing certificate and protected CI secrets, then add signing and timestamping to the release pipeline. Never commit certificate files or passwords to the repository.

## Current scope

This workflow packages the current Windows desktop build and bundled Core Service. It does not yet provide an auto-updater, signed installer, notarization, or a tested upgrade/migration path between installed versions.
