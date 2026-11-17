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
