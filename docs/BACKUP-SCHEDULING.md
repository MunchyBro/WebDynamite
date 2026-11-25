# Automated Backup Scheduling & Retention

WebDynamite can run recurring full-system backups while its Core Service is running.

- Enable or disable the schedule and select an interval from 1 hour to 30 days.
- Choose a retention count from 1 to 100 verified backups.
- The Backups screen reports the next run, last run, result and most recent failure.
- Scheduled backups use the same consistency rules as manual backups: every managed website must be stopped.
- Retention removes only backups that pass integrity verification. At least one verified backup is always retained.
- Schedules are stored in the local SQLite registry and survive application restarts. A missed run is attempted after the Core Service starts again.
- Scheduling does not run while WebDynamite is closed. A Windows service/task integration is not included.
- Backups remain on the same PC unless the user separately configures storage outside WebDynamite.

The scheduler checks for due work every 15 seconds. It records success or failure and advances the next scheduled time after each attempt.

## Backup storage health

The Backups screen includes a storage summary showing the number of registered backups, how many currently pass integrity verification, the total size of the managed backup folders, and free/total capacity for the volume containing the backup directory. Refreshing backup history refreshes this summary too.

The summary verifies each registered backup before counting it as verified. A backup whose files are missing, changed, unsupported, or outside the managed backup directory is reported as needing attention. Storage size is measured from the actual managed backup folders; symbolic links are not followed. The volume capacity values are supplied by the operating system and may be unavailable on unusual or removable filesystems.

A low-space warning is shown when less than 1 GiB is available. This is an advisory threshold, not a reservation or a guarantee that a backup will fit; backup creation still enforces its 10 GiB / 100,000-file limit and reports filesystem write failures.

## Previewing retention cleanup

In the Backups screen, choose the retention count and select **Preview retention cleanup** to see how many currently verified backups would be kept or removed and the approximate storage space that would be recovered. The preview is read-only: it does not delete or modify any backup.

Only backups that pass integrity verification are eligible for automatic retention cleanup. Unverified, missing, or otherwise unavailable backups are reported separately and are protected from retention deletion. The preview reflects the inventory at the time it runs; the scheduled cleanup occurs only after a successful automatic backup and may differ if backup files change before then.


## Restore preflight

The restore preview checks the current website and managed-process state, confirms that each website destination is available, and reports the current website/database data that would be overwritten. It also estimates the size of the automatic pre-restore safety backup and reports free space on the backup volume. Missing destinations, unsupported database entries, or running websites/processes block the restore action until resolved.

Space figures are estimates based on the current files and can change before restore. The restore operation repeats its own integrity, destination and stopped-process checks; the preview is not a reservation of disk space and does not modify any files.
