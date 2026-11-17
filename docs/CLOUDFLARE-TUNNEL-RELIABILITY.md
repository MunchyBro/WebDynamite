# Cloudflare Quick Tunnel reliability

WebDynamite captures Quick Tunnel URLs from both `cloudflared` stdout and stderr because CLI versions and log configurations may emit the generated URL on either stream. Both streams are drained concurrently to avoid blocking the child process on a full pipe.

The Networking view reports the actual in-memory tunnel start timestamp instead of resetting it whenever the status list is refreshed. When a tunnel process exits, its URL and start metadata are cleared from the live registry.

The generated hostname remains temporary and is not persisted across Core Service restarts. `cloudflared` must still be installed separately or selected with `CLOUDFLARED_BIN`.