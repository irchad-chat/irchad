#!/usr/bin/env python3
# Regenerates deploy/ergo.yaml from an upstream ergo default.yaml:
#   curl -sLO https://raw.githubusercontent.com/ergochat/ergo/vX.Y.Z/default.yaml
#   python3 deploy/tools/patch-ergo-default.py default.yaml deploy/ergo.yaml
# Each replacement asserts it matched, so a changed upstream fails loudly.
import sys
src, dst = sys.argv[1], sys.argv[2]
s = open(src).read()
def sub(old, new, count=1):
    global s
    n = s.count(old)
    assert n == count, f"{old!r}: expected {count}, found {n}"
    s = s.replace(old, new)

header = """# IrChad dev ergo config: ergo v2.19.1 default.yaml with IrChad changes.
# Every change is tagged `# irchad:` so it's easy to re-apply on a newer default.yaml.
# DEV ONLY: the oper password and API bearer token below are public.

"""
sub("    name: ergo.test", "    name: chat.localhost # irchad:")
# listeners: plaintext IRC for debugging + plaintext websocket behind Caddy; no TLS (Caddy terminates)
sub('''        "127.0.0.1:6667": # (loopback ipv4, localhost-only)
        "[::1]:6667":     # (loopback ipv6, localhost-only)''',
'''        # irchad: plaintext on all interfaces inside the container; compose only
        # publishes it on host loopback, for raw-client debugging.
        ":6667":
        # irchad: IRC over WebSocket. Caddy terminates TLS and proxies /irc here.
        ":8097":
            websocket: true''')
start = s.index('        ":6697":')
end = s.index('        # Example of a Unix domain socket')
s = s[:start] + "        # irchad: no TLS listener; Caddy terminates TLS.\n\n" + s[end:]
sub('    proxy-allowed-from:\n', '    proxy-allowed-from:\n        - 10.89.77.0/24 # irchad: compose network (Caddy sets X-Forwarded-For)\n')
sub('        # IPs/networks which are exempted from connection limits\n        exempted:\n            - "localhost"\n',
    '        # IPs/networks which are exempted from connection limits\n        exempted:\n            - "localhost"\n            # irchad: dev only. Rootless port forwarding makes every host\n            # connection appear to come from the compose network.\n            - "10.89.77.0/24"\n')
sub('    name: ErgoTest\n', '    name: ChadDev # irchad:\n')
# accounts
sub('        always-on: "opt-in"', '        always-on: "opt-out" # irchad: D11')
sub('        auto-away: "opt-in"', '        auto-away: "opt-out" # irchad: away when no device is connected')
# languages
sub('    path: languages', '    path: /ircd-bin/languages # irchad: path inside the official image')
# opers: dev password "chadadmin"
sub('        password: "$2a$04$0123456789abcdef0123456789abcdef0123456789abcdef01234"\n',
    '        password: "$2a$04$ek1Om4l3EeTgkz5hu.fIIeb0JJBnCsYV19SDmSul/IFagw20Wxagm" # irchad: dev password "chadadmin"\n')
# datastore
sub('    path: ircd.db', '    path: /ircd/ircd.db # irchad:')
sub('''    sqlite:
        enabled: false
        # path to the SQLite database file
        database-path: "ergo_history.db"''', '''    sqlite:
        enabled: true # irchad: persistent history without another service
        # path to the SQLite database file
        database-path: "/ircd/ergo_history.db"''')
# history
sub('''    persistent:
        enabled: false''', '''    persistent:
        enabled: true # irchad:''')
sub('        allow-individual-delete: false', '        allow-individual-delete: true # irchad: needed for REDACT')
# api
sub('''api:
    # is the API enabled at all?
    enabled: false
    # listen address:
    listener: "127.0.0.1:8089"''', '''api:
    # is the API enabled at all?
    enabled: true # irchad: chadd checks credentials here
    # listen address:
    listener: ":8089" # irchad: only reachable on the compose network''')
sub('''    bearer-tokens:
        - "example"''', '''    bearer-tokens:
        - "chad-dev-api-token" # irchad: dev only; must match chadd's config''')
open(dst, "w").write(header + s)
