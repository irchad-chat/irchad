# Production deploy (native, systemd)

Debian 13, no containers (D24). Live at `irchad.chat` (64.177.55.62); the old `chat.gentoo.party` is still served so saved clients and old upload links keep working (D55). SSH alias `chad-prod`.

| Service | Install | Config | Listens |
|---|---|---|---|
| Caddy | Caddy's apt repo | `/etc/caddy/Caddyfile` (from `Caddyfile`), hosts via `IRCHAD_HOST` in `caddy.service.d/irchad.conf` (space-separated: `irchad.chat www.irchad.chat chat.gentoo.party`) | :80, :443 tcp+udp |
| Website | `site/dist` | `/var/www/irchad`, served by Caddy for any path chadd/ergo/LiveKit don't own | — |
| Web app | `web/dist` | `/var/www/irchad-chat`, served by Caddy at `/chat/` (any unknown path there gets `index.html`) | — |
| ergo | release tarball in `/opt/ergo` | `/etc/ergo/ircd.yaml`, state in `/var/lib/ergo` | 127.0.0.1: 6667, 8097 (ws), 8089 (API); public 6697 (TLS, cert copied from Caddy by `ergo-certs.path`) |
| LiveKit | `/usr/local/bin/livekit-server` | `/etc/livekit/livekit.yaml` + `/etc/livekit/env` | 127.0.0.1:7880, public 7881/tcp + 50000-50100/udp |
| chadd | `/usr/local/bin/chadd` | `/etc/chadd/chadd.toml` | 127.0.0.1:8080 |

Secrets (oper password, chadd's oper password, ergo API token, LiveKit secret) were generated on the server and live in `/root/irchad-secrets`. They are never in the repo.
The firewall is `/etc/nftables.conf`: 22, 80, 443, 6697 tcp, 443 udp, 7881 tcp, 50000-50100 udp.

## Updating

- **chadd:** CI's `deploy-chadd` job does this on a push to `main` once the Linux checks pass, but only when chadd's inputs changed since the commit in `/usr/local/bin/chadd.rev` (chadd, chad-proto, `Cargo.toml`/`Cargo.lock`, `.cargo/`, the Dockerfile). It writes that file after each deploy.
  To do it by hand, build against trixie's glibc, then copy the binary over, restart, and record the commit. Without a `chadd.rev` (or with a commit CI doesn't know), the next push deploys.
  ```sh
  podman build -f deploy/chadd.Dockerfile -t irchad-chadd .
  podman cp $(podman create irchad-chadd):/usr/local/bin/chadd ./chadd
  scp chadd chad-prod:/usr/local/bin/chadd.new && ssh chad-prod 'mv /usr/local/bin/chadd.new /usr/local/bin/chadd && systemctl restart chadd'
  git rev-parse HEAD | ssh chad-prod 'cat > /usr/local/bin/chadd.rev'
  ```
- **Website:** `cd site && pnpm build && rsync -a --delete dist/ chad-prod:/var/www/irchad/`.
- **Web app:** `cd web && pnpm build && rsync -a --delete dist/ chad-prod:/var/www/irchad-chat/`. The build ignores `.env.development*` and talks to the origin it's served from.
- **ergo.yaml changes:** regenerate the config on the server, then `systemctl reload ergo`.
  ```sh
  . /root/irchad-secrets
  python3 ergo-prod.py irchad.chat GentooParty "$(caddy hash-password --plaintext "$OPER_PASS")" "$ERGO_API_TOKEN" "$(caddy hash-password --plaintext "$CHADD_OPER_PASS")" < ergo.yaml > /etc/ergo/ircd.yaml
  ```
- **ergo / LiveKit versions:** download the release, check it against the published sha256, swap the binary, then restart.
- **Debian:** `unattended-upgrades` handles security updates.

### Turning on account deletion (D69, once)

```sh
echo "CHADD_OPER_PASS=$(openssl rand -hex 16)" >> /root/irchad-secrets && . /root/irchad-secrets
install -d -o ergo -g chadd -m 2770 /var/lib/ergo/export
# regenerate ircd.yaml as above (it takes the chadd oper hash now), then: systemctl reload ergo
# add [delete] to /etc/chadd/chadd.toml (docs/hosting.md) with oper_password = $CHADD_OPER_PASS, deploy chadd, then:
journalctl -u chadd -o cat | sudo -u chadd python3 upload-owners.py /var/lib/chadd/uploads
```
Messages sent before the reload aren't indexed, so deletion can't find them.

### Turning on Sign in with Discord (D96, once)

```sh
openssl genpkey -algorithm ed25519 -out /etc/chadd/jwt.pem && chown chadd: /etc/chadd/jwt.pem && chmod 600 /etc/chadd/jwt.pem
openssl pkey -in /etc/chadd/jwt.pem -pubout -out /etc/ergo/jwt_pub.pem
install -o chadd -m 600 /dev/stdin /etc/chadd/discord-secret   # paste the Discord app's client secret, then ^D
# regenerate ircd.yaml as above (jwt-auth turns on now that jwt_pub.pem exists), systemctl reload ergo,
# add [discord] to /etc/chadd/chadd.toml (docs/hosting.md), then: systemctl restart chadd
```
The Discord app's OAuth2 redirect must be `https://irchad.chat/api/oauth/discord/callback`.

Raw IRC for admins: `ssh -L 6667:127.0.0.1:6667 chad-prod`, then connect a client to `localhost:6667` and `/OPER admin <OPER_PASS>`.

## App store reviewers

Reviewers log in to the live network as `reviewer` and `reviewer2` (two, so they can try DMs and calls).
Reviewers often test "Delete account", so (re)create both before each submission. The script leaves working accounts alone:

```sh
ssh -L 6667:127.0.0.1:6667 chad-prod    # in another terminal
OPER_PASS=$(ssh chad-prod "sed -n 's/^OPER_PASS=//p' /root/irchad-secrets") REVIEW_PASS=<password> python3 deploy/tools/reviewer.py
```

Review notes for App Store Connect and Play Console (Play: App access → "All or some functionality is restricted"):

> IrChad is a chat client. To sign in: open the app, enter `https://irchad.chat/invite` as the invite link, choose **Log in** (not Sign up), and use `reviewer` / `<password>`.
> A second account, `reviewer2` / `<password>`, can be used on another device to try direct messages, voice calls and screen sharing.
> Account deletion: tap your name → Leave irchad.chat… → Delete account. Steps: https://irchad.chat/delete-account
