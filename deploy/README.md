# Dev server stack

ergo (IRC) + LiveKit (media) + chadd (discovery, auth) behind Caddy. **Dev only:** every secret in here is public.

```sh
docker compose -f deploy/compose.yaml up -d --build   # or: podman compose ...
python3 deploy/seed.py                                # accounts + channels, idempotent
curl -k https://chat.localhost:8443/.well-known/irchad.json
```

| What | Where |
|---|---|
| Origin | `https://chat.localhost:8443` (`IRCHAD_HTTPS_PORT`, `IRCHAD_HOST` in `deploy/.env`; see `.env.example`) |
| Invite page | `https://chat.localhost:8443/invite` |
| Raw IRC (debugging) | `127.0.0.1:6667`, plaintext |
| Second network | `https://chat2.localhost:8443` (`IRCHAD_HOST2`): its own ergo (`ChadDev2`, raw IRC `127.0.0.1:6668`) and chadd, sharing Caddy (same CA root) and LiveKit (so `#dev` on both networks is the same voice room). Seed with `python3 deploy/seed.py --port 6668` |
| Accounts | `alice` / `alicepass`, `bob` / `bobpass` |
| Channels | `#general`, `#random`, `#dev` (registered to alice) |
| ergo oper | `admin` / `chadadmin` |
| ergo HTTP API | `http://ergo:8089` on the compose network only, bearer `chad-dev-api-token` |
| LiveKit | key `devkey`, secret in `livekit.yaml`. Media on UDP 50000-50100 + TCP 7881, advertised as `LIVEKIT_NODE_IP` (default 127.0.0.1) |

`.localhost` names resolve to loopback in curl and browsers, but glibc only resolves them with `nss-myhostname`.
IrChad itself maps `*.localhost` to 127.0.0.1 (RFC 6761), for both discovery and the WebSocket. If another tool can't resolve `chat.localhost`, add it to `/etc/hosts`.

Caddy's internal CA root is in the `caddy-data` volume (`/data/caddy/pki/authorities/local/root.crt`) if you want to trust it.

## Files

- `ergo.yaml` is ergo v2.19.1's `default.yaml` with IrChad changes, each tagged `# irchad:`.
  To move to a new ergo version, run `tools/patch-ergo-default.py` against the new `default.yaml`. It fails loudly if upstream changed under a patch.
- `livekit.yaml`: LiveKit dev config. Production TURN settings are in the comments.
- `Caddyfile`: routing per `docs/ARCHITECTURE.md`.
- Uploads go to the `chadd-uploads` volume (`chadd2-uploads` for the second network). ergo advertises them to plain IRC clients as ISUPPORT `draft/FILEHOST` (the `ERGO__SERVER__ADDITIONAL_ISUPPORT` env in `compose.yaml`).
- Soundboard clips are the files in `sounds/`, mounted read-only into both chadds (`[sounds]` in their configs). Add or remove files at will; clients list them each time the soundboard opens (D37).
- `chadd.toml`: chadd config. compose sets `CHADD_ORIGIN` to match the published host and port. `[ergo]` holds the API URL and bearer token (must match `ergo.yaml`), and `[livekit]` holds the key and secret (must match `livekit.yaml`).
- `tools/ergo-probe.py`: the Phase 0 capability probe (see `docs/notes/ergo-caps.md`).

Port 443 needs `sysctl net.ipv4.ip_unprivileged_port_start=443` for rootless containers. Then set `IRCHAD_HTTPS_PORT=443`.
