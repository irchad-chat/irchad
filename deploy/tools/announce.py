#!/usr/bin/env python3
"""Announce a release in #chad as `chadster` (release.yml's publish job).

Usage: CHADSTER_PASSWORD=... announce.py <version> <release url>
"""

import os
import sys

from irc import Irc

HOST, PORT, NICK, CHANNEL = "irchad.chat", 6697, "chadster", "#chad"

version, url = sys.argv[1:3]
irc = Irc(HOST, PORT, tls=True)
_, ok = irc.connect(NICK, password=os.environ["CHADSTER_PASSWORD"])
if not ok:
    sys.exit("SASL login failed")
irc.send(f"JOIN {CHANNEL}")
irc.until(lambda l, c: (c == "JOIN" and l.lstrip(":").split("!")[0] == NICK) or c in ("403", "405", "471", "473", "474", "475", "477"))
irc.send(f"PRIVMSG {CHANNEL} :IrChad {version} is out: {url}")
irc.quit()
