# Cloudflare Quick Tunnel publishing

WebDynamite can start one Cloudflare Quick Tunnel for an individual registered website using the locally installed `cloudflared` executable.

## Setup
- Install cloudflared separately and make it available on PATH, or set `CLOUDFLARED_BIN` to its executable.
- The tunnel command is `cloudflared tunnel --url <local-origin> --no-autoupdate`.
- Quick Tunnels produce temporary `trycloudflare.com` hostnames. They are for previews, demos and testing; they are not a substitute for a configured named tunnel and production DNS.

## Behavior
- Static sites route through the Core Service's website-specific static path.
- Node.js, PHP and Python sites route to their allocated loopback runtime port and must be running before tunnel start.
- Tunnel processes are per website, visible in Networking, and can be stopped individually.
- The Core Service kills tunnel processes on shutdown.
- The management API remains bound to loopback and is never used as the tunnel origin.

## Limitations
- Quick Tunnel URLs are ephemeral and are not persisted as stable hostnames.
- This feature does not configure Cloudflare DNS, custom domains, access policies, named tunnels, or Cloudflare account credentials.
- Availability depends on Cloudflare and the local network; no uptime guarantee is implied.


## Tunnel process diagnostics

Each website has a separate tunnel log under `logs/tunnels/`. Both cloudflared stdout and stderr are captured. Retained output is capped at 512 KiB per tunnel start; the log is recreated on the next start. Networking displays the log path beside each tunnel.

Tunnel process inspection errors no longer cause WebDynamite to discard the child-process handle and potentially orphan a running process. Stopping a tunnel now reports process inspection, termination or reaping failures instead of silently claiming success. The log is diagnostic output and may contain temporary public tunnel URLs; protect access to the WebDynamite data directory.
