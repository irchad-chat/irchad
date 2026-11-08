#!/usr/bin/env python3
"""(Re)create the app store reviewer accounts. Idempotent; rerun before each submission.

    ssh -L 6667:127.0.0.1:6667 chad-prod      # in another terminal
    OPER_PASS=... REVIEW_PASS=... python3 deploy/tools/reviewer.py

Accounts: reviewer, reviewer2 (two, so DMs and calls can be tried), both with REVIEW_PASS.
Reviewers test "Delete account", which leaves the name reserved, so a missing or
broken account is ERASEd (that drops the reservation) and registered again.
"""

import argparse
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from irc import Irc, command, params  # noqa: E402

ACCOUNTS = ["reviewer", "reviewer2"]


def nickserv(c, line):
    """Send a NickServ command and return all its reply notices, joined."""
    c.send(f"NS {line}")
    res = c.until(lambda l, cmd: cmd == "NOTICE" and l.startswith(":NickServ!")) + c.drain()
    return " ".join(params(l)[-1] for l in res if command(l) == "NOTICE" and l.startswith(":NickServ!"))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--host", default="127.0.0.1")
    ap.add_argument("--port", type=int, default=6667)
    ap.add_argument("--oper", default="admin")
    args = ap.parse_args()
    oper_pass, password = os.environ.get("OPER_PASS"), os.environ.get("REVIEW_PASS")
    if not oper_pass or not password:
        sys.exit("set OPER_PASS and REVIEW_PASS")

    admin = Irc(args.host, args.port)
    admin.connect("reviewer-setup")
    admin.send(f"OPER {args.oper} {oper_pass}")
    res = admin.until(lambda l, cmd: cmd in ("381", "400", "491", "464"))
    if command(res[-1]) not in ("381", "400"):
        sys.exit(f"OPER failed: {res[-1]}")
    admin.drain()

    for name in ACCOUNTS:
        c = Irc(args.host, args.port)
        _, ok = c.connect(name, password)
        c.quit()
        if ok:
            print(f"{name}: logs in, left alone")
            continue
        # ERASE asks for a confirmation code, the last word of its reply.
        reply = nickserv(admin, f"ERASE {name}")
        code = reply.split()[-1].rstrip(".")
        if "confirm" in reply.lower():
            print(f"{name}: erase: {nickserv(admin, f'ERASE {name} {code}')}")
        reply = nickserv(admin, f"SAREGISTER {name} {password}")
        print(f"{name}: {reply}")
        c = Irc(args.host, args.port)
        _, ok = c.connect(name, password)
        c.quit()
        if not ok:
            sys.exit(f"{name}: still can't log in")
    admin.quit()


if __name__ == "__main__":
    main()
