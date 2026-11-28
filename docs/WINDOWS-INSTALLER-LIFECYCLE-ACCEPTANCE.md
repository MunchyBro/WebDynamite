# Windows Installer Lifecycle Acceptance

The Windows Installer workflow exercises the actual NSIS artifact on a disposable, elevated GitHub-hosted Windows runner.

## Automated scenario

1. Refuse to run if the canonical `WebDynamiteCore` service already exists.
2. Create a unique installation destination under Program Files and a unique preservation marker under `%ProgramData%\WebDynamite\LifecycleAcceptance`.
3. Run the built installer silently and verify the desktop executable and lifecycle resources were installed.
4. Run the same installer again into the same destination to exercise the NSIS preinstall and postinstall repair/upgrade hooks.
5. Run the installed NSIS uninstaller silently.
6. Verify the desktop executable is removed, the Core Service is not registered, and the ProgramData marker remains unchanged.

The test removes only its own unique marker directory. If a failure occurs before uninstall completes, it leaves the isolated install directory in place on the ephemeral runner for diagnostics.

## Scope and limitations

The CI artifact is an unsigned engineering build. Per the production security policy, it installs the desktop application but does not register or activate the privileged Core Service. The test therefore validates real NSIS installation, in-place repair/upgrade hook execution, uninstallation, and data preservation without production signing secrets.

It does **not** replace a signed-certificate acceptance run or a clean Windows VM test of production service activation, reboot startup, service crash recovery, application-data migrations, or rollback from a failed signed upgrade. Those remain explicit release acceptance requirements.
