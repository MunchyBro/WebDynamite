# Cloudflare Quick Tunnel reliability

Tunnel metadata from a previous process is cleared before stdout/stderr reader threads for a replacement process are started. This ordering prevents a fast `cloudflared` URL announcement from being overwritten by stale-state cleanup.

Both stdout and stderr are drained concurrently because CLI versions and logging configurations may emit the generated URL on either stream. Start timestamps are held in the live process registry and remain stable across status refreshes. Exited or stopped processes have URL and start metadata cleared.

Quick Tunnel hostnames remain temporary and are not persisted across Core Service restarts. `cloudflared` must be installed separately or selected with `CLOUDFLARED_BIN`.

Tunnel startup is considered ready only after cloudflared announces a `.trycloudflare.com` URL on stdout or stderr. The API waits up to 10 seconds, checks for early process exit during that window, and returns the URL only after it is captured. If the process exits early, the start request fails with the log path. If no URL is announced before the deadline, WebDynamite terminates and reaps the newly started process, clears its in-memory metadata, and returns a timeout error. This readiness check confirms URL assignment only; it does not prove that the website origin is healthy or that the public route is reachable.


## Unexpected process-exit supervision

While the Core Service is running, a lightweight supervisor checks the Caddy and cloudflared child-process handles every two seconds. When Windows (or the host OS) reports that a child has exited, the supervisor reaps the handle and removes its associated in-memory running metadata. For tunnels this includes the temporary URL and start timestamp, so a later status read cannot present a dead tunnel as active. Caddy's process slot is cleared in the same way. Process-inspection errors are treated conservatively: the handle and metadata are retained rather than guessed to be stopped.

Automatic recovery is available as an explicit opt-in under Settings → Automatic hosting recovery and is disabled by default. While enabled, an unexpected Caddy exit is retried through the same configuration validation and local-admin readiness gate as a manual start. Unexpected Quick Tunnel exits are retried through the normal URL-announcement readiness path; each replacement receives a new temporary hostname. Recovery uses a configurable delay (5–300 seconds), a bounded retry budget (1–5 attempts), and per-process in-flight protection. A successful restart earns a fresh retry budget only after five minutes of stable operation. Manual stop requests suppress pending recovery; starting a process manually clears that one-shot suppression. Disabling global public exposure stops tunnels and suppresses their recovery. Clean exits are not restarted.

Recovery runs only while the Core Service is alive; it does not provide recovery after sign-out, reboot, or Core Service shutdown. It does not prove public reachability, restore a previous Quick Tunnel URL, or guarantee that an origin website is healthy. Recovery events are retained in the local database (newest 200) and the Settings page can display the latest 100. Settings also shows a manually refreshed process-registry snapshot for Caddy and configured Quick Tunnels with each process's most recent recovery event. These snapshots do not establish website health or public reachability. There are no OS/email notifications.
