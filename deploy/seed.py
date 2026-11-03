#!/usr/bin/env python3
"""Seed the dev stack: test accounts and registered channels. Idempotent.

    python3 deploy/seed.py [--host 127.0.0.1] [--port 6667]

Accounts (password = name + "pass"): alice, bob.
alice owns the channels; she uses the dev oper to get ops first.
"""

import argparse
import os
import sys

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "tools"))
from irc import Irc, command, params  # noqa: E402

ACCOUNTS = ["alice", "bob"]
OPER = ("admin", "chadadmin")  # from deploy/ergo.yaml (dev only)
CHANNELS = {
    "#general": "General chat",
    "#random": "Off topic",
    "#dev": "IrChad development",
}


def ensure_account(host, port, nick):
    password = nick + "pass"
    c = Irc(host, port)
    _, ok = c.connect(nick, password)
    if ok:
        print(f"account {nick}: exists")
        return c
    c.quit()
    c = Irc(host, port)
    c.connect(nick)
    c.send(f"REGISTER * * {password}")
    res = c.until(lambda l, cmd: cmd in ("REGISTER", "FAIL"))
    if command(res[-1]) == "FAIL":
        sys.exit(f"account {nick}: registration failed: {res[-1]}")
    print(f"account {nick}: registered")
    return c


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--host", default="127.0.0.1")
    ap.add_argument("--port", type=int, default=6667)
    args = ap.parse_args()

    conns = {nick: ensure_account(args.host, args.port, nick) for nick in ACCOUNTS}
    alice = conns["alice"]
    alice.send(f"OPER {OPER[0]} {OPER[1]}")
    res = alice.until(lambda l, cmd: cmd in ("381", "400", "491", "464"))
    if command(res[-1]) not in ("381", "400"):  # 400: already opered (always-on)
        sys.exit(f"OPER failed: {res[-1]}")
    for chan, topic in CHANNELS.items():
        # SAMODE as the dev oper, so registering doesn't depend on who holds ops.
        alice.send(f"JOIN {chan}")
        alice.send(f"SAMODE {chan} +o alice")
        alice.send(f"CS REGISTER {chan}")
        res = alice.until(lambda l, cmd: cmd == "NOTICE" and l.startswith(":ChanServ!"))
        print(f"ChanServ {chan}: {params(res[-1])[-1]}")
        alice.send(f"TOPIC {chan} :{topic}")
        for nick, c in conns.items():
            if nick != "alice":
                c.send(f"JOIN {chan}")
    # The official list, on chadd's `home` channel: IrChad joins and parts to match.
    alice.send(f"METADATA #general SET chad/channels :{' '.join(CHANNELS)}")
    for c in conns.values():
        c.drain()
    for c in conns.values():
        c.quit()


if __name__ == "__main__":
    main()
