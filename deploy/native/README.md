# Production deploy (native, systemd)

Debian 13, no containers (D24). Live at `chat.gentoo.party` (64.177.55.62). SSH alias `chad-prod`.

| Service | Install | Config | Listens |
|---|---|---|---|
| Caddy | Caddy's apt repo | `/etc/caddy/Caddyfile` (from `Caddyfile`), host via `caddy.service.d/irchad.conf` | :80, :443 tcp+udp |
| ergo | release tarball in `/opt/ergo` | `/etc/ergo/ircd.yaml`, state in `/var/lib/ergo` | 127.0.0.1: 6667, 8097 (ws), 8089 (API) |
| LiveKit | `/usr/local/bin/livekit-server` | `/etc/livekit/livekit.yaml` + `/etc/livekit/env` | 127.0.0.1:7880, public 7881/tcp + 50000-50100/udp |
| chadd | `/usr/local/bin/chadd` | `/etc/chadd/chadd.toml` | 127.0.0.1:8080 |

Secrets (oper password, ergo API token, LiveKit secret) were generated on the server and live in `/root/irchad-secrets`. They are never in the repo.
The firewall is `/etc/nftables.conf`: 22, 80, 443 tcp, 443 udp, 7881 tcp, 50000-50100 udp.

## Updating

- **chadd:** build against trixie's glibc, then copy the binary over and restart.
  ```sh
  podman build -f deploy/chadd.Dockerfile -t irchad-chadd .
  podman cp $(podman create irchad-chadd):/usr/local/bin/chadd ./chadd
  scp chadd chad-prod:/usr/local/bin/chadd && ssh chad-prod systemctl restart chadd
  ```
- **ergo.yaml changes:** regenerate the config on the server, then `systemctl reload ergo`.
  ```sh
  . /root/irchad-secrets
  python3 ergo-prod.py chat.gentoo.party GentooParty "$(caddy hash-password --plaintext "$OPER_PASS")" "$ERGO_API_TOKEN" < ergo.yaml > /etc/ergo/ircd.yaml
  ```
- **ergo / LiveKit versions:** download the release, check it against the published sha256, swap the binary, then restart.
- **Debian:** `unattended-upgrades` handles security updates.

Raw IRC for admins: `ssh -L 6667:127.0.0.1:6667 chad-prod`, then connect a client to `localhost:6667` and `/OPER admin <OPER_PASS>`.
