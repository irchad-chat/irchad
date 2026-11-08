#!/usr/bin/env python3
"""Derive a production ircd.yaml from deploy/ergo.yaml (stdin -> stdout).

usage: ergo-prod.py <server-name> <network-name> <oper-bcrypt-hash> <api-token> <chadd-oper-bcrypt-hash> < ergo.yaml

Every replacement must match exactly once, so a changed ergo.yaml fails loudly.
"""

import sys

host, network, oper_hash, token, chadd_hash = sys.argv[1:6]

REPLACE = [
    ("    name: ChadDev # irchad:", f"    name: {network}"),
    ("    name: chat.localhost # irchad:", f"    name: {host}"),
    # Loopback only: Caddy proxies /irc, and raw IRC is for SSH-tunnelled admins.
    ('        ":6667":\n', '        "127.0.0.1:6667":\n'),
    ('        ":8097":\n', '        "127.0.0.1:8097":\n'),
    # Public TLS for plain IRC clients, using Caddy's Let's Encrypt cert (copied by ergo-certs.service).
    ("        # irchad: no TLS listener; Caddy terminates TLS.\n",
     '        ":6697":\n            tls:\n                cert: /var/lib/ergo/tls/fullchain.pem\n                key: /var/lib/ergo/tls/privkey.pem\n'),
    ("        - 10.89.77.0/24 # irchad: compose network (Caddy sets X-Forwarded-For)\n", ""),
    ('            - "10.89.77.0/24"\n', ""),
    ('lock-file: "ircd.lock"', 'lock-file: "/var/lib/ergo/ircd.lock"'),
    ("    path: /ircd/ircd.db # irchad:", "    path: /var/lib/ergo/ircd.db"),
    ('database-path: "/ircd/ergo_history.db"', 'database-path: "/var/lib/ergo/ergo_history.db"'),
    ("    path: /ircd-bin/languages # irchad: path inside the official image", "    path: /opt/ergo/languages"),
    ('    listener: ":8089" # irchad: only reachable on the compose network', '    listener: "127.0.0.1:8089"'),
    ('        - "chad-dev-api-token" # irchad: dev only; must match chadd\'s config', f'        - "{token}"'),
    ('    output-path: "/ircd/export" #', '    output-path: "/var/lib/ergo/export" #'),
    ('        #"draft/FILEHOST": "https://example.com/filehost"', f'        "draft/FILEHOST": "https://{host}/api/upload"'),
]

text = sys.stdin.read()
for old, new in REPLACE:
    if text.count(old) != 1:
        sys.exit(f"ergo-prod: expected exactly one match for {old!r}, found {text.count(old)}")
    text = text.replace(old, new)

# The oper hash line carries the dev password in its comment; replace the whole line.
lines = text.splitlines(keepends=True)
hits = [i for i, l in enumerate(lines) if '# irchad: dev password "chadadmin"' in l]
if len(hits) != 1:
    sys.exit(f"ergo-prod: expected one oper password line, found {len(hits)}")
lines[hits[0]] = f'        password: "{oper_hash}"\n'
hits = [i for i, l in enumerate(lines) if '# irchad: dev chadd oper password' in l]
if len(hits) != 1:
    sys.exit(f"ergo-prod: expected one chadd oper password line, found {len(hits)}")
lines[hits[0]] = f'        password: "{chadd_hash}"\n'
sys.stdout.write("".join(lines))
