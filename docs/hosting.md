# Host your own network in 15 minutes

An IrChad network is four small programs behind one domain:

| Program | Does | Listens on |
|---|---|---|
| **Caddy** | HTTPS for everything, with automatic certificates | 80, 443 (tcp + udp) |
| **ergo** | IRC: chat, accounts, history | loopback only |
| **LiveKit** | voice and screen share | 7881/tcp, 50000–50100/udp (media), loopback for signalling |
| **chadd** | invite page, discovery, logins for voice, uploads, link previews | loopback only |

These steps follow our own production box (`deploy/native/`, Debian 13, 1 vCPU / 2 GB RAM). Any recent Linux with systemd works the same way.

## 1. DNS and firewall (2 min)

- Point an `A` (and `AAAA`, if you have IPv6) record for your chat domain, say `chat.example.com`, at the server.
- Open these ports and nothing else: `22/tcp` (SSH), `80/tcp`, `443/tcp`, `443/udp`, `7881/tcp`, `50000-50100/udp`.
  The UDP range is the voice traffic itself. If it's blocked, calls fall back to `7881/tcp`, which is slower.
  Behind NAT, forward the same ports.

## 2. Install the programs (5 min)

```sh
# Caddy, from its apt repo: https://caddyserver.com/docs/install#debian-ubuntu-raspbian
apt install caddy

# ergo ≥ 2.19 (D9): unpack the release into /opt/ergo
curl -LO https://github.com/ergochat/ergo/releases/download/v2.19.1/ergo-2.19.1-linux-x86_64.tar.gz
mkdir -p /opt/ergo && tar xzf ergo-*.tar.gz -C /opt/ergo --strip-components=1

# LiveKit server
curl -sSL https://get.livekit.io | bash        # installs /usr/local/bin/livekit-server

# chadd: copy the binary out of the release image
podman create --name chadd-tmp git.gentoo.party/sam/chadd:latest
podman cp chadd-tmp:/usr/local/bin/chadd /usr/local/bin/chadd && podman rm chadd-tmp

useradd --system --home /var/lib/ergo ergo
useradd --system --home /var/lib/chadd chadd
```

## 3. Configure (5 min)

Generate the secrets once and keep them out of git:

```sh
cat > /root/irchad-secrets <<EOF
OPER_PASS=$(openssl rand -hex 12)
ERGO_API_TOKEN=$(openssl rand -hex 32)
LIVEKIT_SECRET=$(openssl rand -hex 32)
EOF
chmod 600 /root/irchad-secrets
. /root/irchad-secrets
```

From a checkout of this repo, copy the files over and fill them in:

- **Caddy:** `deploy/native/Caddyfile` to `/etc/caddy/Caddyfile`, and set `IRCHAD_HOST=chat.example.com` in the service's environment
  (`systemctl edit caddy`, then `Environment=IRCHAD_HOST=chat.example.com`).
- **ergo:** generate `/etc/ergo/ircd.yaml` from the dev config. The script swaps in your names, loopback listeners, secrets and the `draft/FILEHOST` upload URL:
  ```sh
  python3 deploy/native/ergo-prod.py chat.example.com MyNetwork \
    "$(caddy hash-password --plaintext "$OPER_PASS")" "$ERGO_API_TOKEN" \
    < deploy/ergo.yaml > /etc/ergo/ircd.yaml
  ```
- **LiveKit:** `deploy/native/livekit.yaml` to `/etc/livekit/livekit.yaml`, and `/etc/livekit/env` containing
  `LIVEKIT_KEYS="irchad: <LIVEKIT_SECRET>"` and `NODE_IP=<the server's public IP>`.
- **chadd:** `/etc/chadd/chadd.toml`:
  ```toml
  listen = "127.0.0.1:8080"
  origin = "https://chat.example.com"
  name = "My Network"
  registration = "open"            # or "closed": accounts only by an admin

  [ergo]
  api = "http://127.0.0.1:8089"
  token = "<ERGO_API_TOKEN>"

  [livekit]
  key = "irchad"
  secret = "<LIVEKIT_SECRET>"

  # Leave this out to turn uploads (and avatars) off.
  [uploads]
  dir = "/var/lib/chadd/uploads"
  max_bytes = 26214400             # 25 MB

  # Optional: the GIF picker. Get a key at https://partner.klipy.com/api-keys
  # (a test key allows 100 searches an hour across the whole network).
  [gifs]
  key = "<KLIPY_API_KEY>"
  # content_filter = "medium"      # off, low, medium or high
  # api = "https://api.klipy.com/v2/"  # any Tenor v2-shaped API
  # provider = "KLIPY"             # shown as "Powered by …"
  ```
- **systemd:** copy `deploy/native/*.service` to `/etc/systemd/system/`, then
  `systemctl daemon-reload && systemctl enable --now ergo livekit chadd caddy`.

## 4. Check it (1 min)

```sh
curl https://chat.example.com/.well-known/irchad.json    # discovery document
```

Open `https://chat.example.com/invite` in a browser. That page is your invite link: send it to friends.
It has a "Click to Join" button and a download button for their OS.
Add `?channel=%23general` to drop them in a channel.

## Running it

You administer the network from a normal IRC client, since there's no admin UI yet.
Raw IRC listens on loopback only, so tunnel it: `ssh -L 6667:127.0.0.1:6667 chat.example.com`, connect your client to `localhost:6667`, then:

| To | Send |
|---|---|
| Become an operator | `/OPER admin <OPER_PASS>` |
| Make a channel and own it | `/JOIN #general`, then `/msg ChanServ REGISTER #general` |
| Set the topic | `/TOPIC #general :Welcome!` |
| Turn voice off in a channel | `/quote METADATA #general SET chad/voice off` (D19) |
| Ban someone from the network | `/UBAN ADD <nick> DURATION 1d REASON <why>` |
| Delete an account | `/msg NickServ SAUNREGISTER <account>` |
| Stop sign-ups | `registration = "closed"` in chadd.toml, plus `accounts.registration.enabled: false` in ircd.yaml |

New channels are auto-joined by everyone who signs up afterwards (IrChad joins everything in `LIST` on first login).

**Uploads** pile up in `uploads.dir` and are never deleted. Watch the disk, or prune old ones:
`find /var/lib/chadd/uploads -mindepth 1 -maxdepth 1 -mtime +90 -exec rm -rf {} +`.
Anyone with an account can upload, at most 30 files a minute.

**Link previews** are fetched by chadd from your server's IP, never from your users' (D25). chadd refuses private and loopback addresses.

**TURN:** some office and campus networks block UDP. To serve them, turn on LiveKit's embedded TURN (see the comments in `deploy/livekit.yaml`) and open its ports.

**Updating:** swap a binary and restart its service. See `deploy/native/README.md`. Users' clients tell them when a new IrChad release is out.
