# Cloudflare Quick Tunnel reliability

Tunnel metadata from a previous process is cleared before stdout/stderr reader threads for a replacement process are started. This ordering prevents a fast `cloudflared` URL announcement from being overwritten by stale-state cleanup.

Both stdout and stderr are drained concurrently because CLI versions and logging configurations may emit the generated URL on either stream. Start timestamps are held in the live process registry and remain stable across status refreshes. Exited or stopped processes have URL and start metadata cleared.

Quick Tunnel hostnames remain temporary and are not persisted across Core Service restarts. `cloudflared` must be installed separately or selected with `CLOUDFLARED_BIN`.