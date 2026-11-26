# Cloudflare Quick Tunnel reliability

Tunnel metadata from a previous process is cleared before stdout/stderr reader threads for a replacement process are started. This ordering prevents a fast `cloudflared` URL announcement from being overwritten by stale-state cleanup.

Both stdout and stderr are drained concurrently because CLI versions and logging configurations may emit the generated URL on either stream. Start timestamps are held in the live process registry and remain stable across status refreshes. Exited or stopped processes have URL and start metadata cleared.

Quick Tunnel hostnames remain temporary and are not persisted across Core Service restarts. `cloudflared` must be installed separately or selected with `CLOUDFLARED_BIN`.

Tunnel startup is considered ready only after cloudflared announces a `.trycloudflare.com` URL on stdout or stderr. The API waits up to 10 seconds, checks for early process exit during that window, and returns the URL only after it is captured. If the process exits early, the start request fails with the log path. If no URL is announced before the deadline, WebDynamite terminates and reaps the newly started process, clears its in-memory metadata, and returns a timeout error. This readiness check confirms URL assignment only; it does not prove that the website origin is healthy or that the public route is reachable.
