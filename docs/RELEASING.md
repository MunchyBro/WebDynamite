# Windows releases

WebDynamite builds a Windows NSIS installer through the **Windows Installer** GitHub Actions workflow.

## Build an installer manually

1. Open **Actions → Windows Installer** in the GitHub repository.
2. Select **Run workflow** on the default branch.
3. Wait for the Windows build to finish.
4. Download the WebDynamite-Windows-Installer artifact from the completed run.

The workflow builds the Core Service first so Tauri can bundle webdynamite-core.exe into the desktop application.

## Publish a versioned release

Push a version tag such as v0.1.0. The workflow builds the installer, uploads it as a workflow artifact, and attaches the installer to a GitHub Release with generated release notes.

Keep the tag version aligned with the application versions in package.json, src-tauri/Cargo.toml, and src-tauri/tauri.conf.json.

## Signing status

Installers produced by the current workflow are **unsigned**. Do not treat an unsigned installer as a production-trustworthy distribution. Phase 118 adds preflight validation for a real Authenticode Code Signing certificate and its pinned SHA-256 identity, but it does not yet sign the desktop executable, Core Service, or installer. The release pipeline must not be considered production-ready until all three signing stages, timestamping, and signature verification are implemented and tested.

See WINDOWS-CODE-SIGNING.md for certificate requirements, pin calculation, preflight validation, and protected CI secret names. Never commit certificate files or passwords to the repository.

## Current scope

This workflow packages the current Windows desktop build and bundled Core Service. It does not yet provide an auto-updater, signed installer, notarization, or a tested upgrade/migration path between installed versions.
