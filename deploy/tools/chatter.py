#!/usr/bin/env python3
"""Synthetic chat load for profiling IrChad against the dev stack.

    python3 deploy/tools/chatter.py [--bots 20] [--rate 10] [--minutes 0] [--seed 1]

Bots join #load1..#load3 and talk: plain lines, long lines, URLs, formatting,
/me, mentions of --mention, replies, edits, reactions, typing, plus some
join/part/nick churn. The same --seed, --bots and --rate give the same mix
and pace of events (which message a reply or edit targets depends on
timing), so before/after profiles are comparable.
Ergo fakelag allows ~2 lines/s per bot, so keep rate/bots under ~1.5.
"""

import argparse
import asyncio
import random
import sys
import time

from irc import command, params

WORDS = ("the build is green again anyone tried the new voice codec rust egui wgpu "
         "pipewire portal lag ping latency patch merged review lgtm ship it nope "
         "works on my machine gentoo kernel compile coffee tonight tomorrow").split()
EMOJI = ["👍", "😂", "🔥", "👀", "🎉", "❤️", "🦀"]


class Bot:
    def __init__(self, nick, reader, writer):
        self.nick, self.r, self.w = nick, reader, writer
        self.mine = []    # msgids of our own messages (echo-message), for edits
        self.seen = []    # msgids of anyone's messages, for replies and reactions
        self.chans = set()

    def send(self, line):
        self.w.write(line.encode() + b"\r\n")

    async def read_loop(self):
        # Must keep reading: every bot receives all channel traffic, and an
        # unread socket fills ergo's sendq and gets the bot killed.
        while line := (await self.r.readline()).decode(errors="replace").rstrip("\r\n"):
            if line.startswith("PING "):
                self.send("PONG " + line[5:])
            elif command(line) == "PRIVMSG" and line.startswith("@"):
                tags = dict(t.partition("=")[::2] for t in line[1:].split(" ", 1)[0].split(";"))
                if mid := tags.get("msgid"):
                    self.seen = self.seen[-50:] + [mid]
                    if line.split(" ", 2)[1].lstrip(":").split("!")[0] == self.nick:
                        self.mine = self.mine[-20:] + [mid]
        print(f"{self.nick}: disconnected", file=sys.stderr)


async def connect(host, port, nick):
    r, w = await asyncio.open_connection(host, port)
    b = Bot(nick, r, w)
    b.send("CAP REQ :message-tags echo-message server-time")
    b.send(f"NICK {nick}")
    b.send(f"USER {nick} 0 * :load bot")
    b.send("CAP END")
    while True:
        line = (await r.readline()).decode(errors="replace")
        if not line:
            raise ConnectionError(f"{nick}: closed during registration")
        if line.startswith("PING "):
            b.send("PONG " + line[5:].strip())
        if command(line) in ("001", "433", "ERROR"):
            if command(line) != "001":
                raise ConnectionError(f"{nick}: {line.strip()}")
            return b


def text(rng, mention):
    words = " ".join(rng.choices(WORDS, k=rng.randint(2, 14)))
    kind = rng.random()
    if kind < 0.05:
        return " ".join(rng.choices(WORDS, k=80))  # long line, wraps a lot
    if kind < 0.10:
        return f"look https://example.com/{rng.randint(1, 10**6)} {words}"
    if kind < 0.14:
        return f"\x02{words}\x02 \x0304,01colour\x03 \x1ditalic\x1d"
    if kind < 0.18:
        return f"\x01ACTION {words}\x01"
    if kind < 0.22 and mention:
        return f"{mention}: {words}"
    if kind < 0.27:
        return f"{words} {rng.choice(EMOJI)}"
    return words


def act(rng, bots, chans, mention):
    b = rng.choice(bots)
    c = rng.choice(sorted(b.chans) or chans)
    roll = rng.random()
    if roll < 0.03:  # churn
        if c in b.chans and len(b.chans) > 1:
            b.send(f"PART {c} :brb")
            b.chans.discard(c)
        else:
            b.send(f"JOIN {c}")
            b.chans.add(c)
    elif roll < 0.04:
        b.nick = f"{b.nick.split('_')[0]}_{rng.randint(0, 99)}"
        b.send(f"NICK {b.nick}")
    elif roll < 0.10:
        b.send(f"@+typing=active TAGMSG {c}")
    elif roll < 0.16 and b.seen:
        b.send(f"@+draft/react={rng.choice(EMOJI)};+draft/reply={rng.choice(b.seen)} TAGMSG {c}")
    elif roll < 0.20 and b.mine:
        b.send(f"@+chad/edit={rng.choice(b.mine)} PRIVMSG {c} :{text(rng, None)} (edited)")
    elif roll < 0.28 and b.seen:
        b.send(f"@+draft/reply={rng.choice(b.seen)} PRIVMSG {c} :{text(rng, mention)}")
    else:
        b.send(f"PRIVMSG {c} :{text(rng, mention)}")


async def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--host", default="127.0.0.1")
    ap.add_argument("--port", type=int, default=6667)
    ap.add_argument("--bots", type=int, default=20)
    ap.add_argument("--rate", type=float, default=10, help="events per second, all bots together")
    ap.add_argument("--channels", default="#load1,#load2,#load3")
    ap.add_argument("--mention", default="", help="nick to highlight now and then")
    ap.add_argument("--minutes", type=float, default=0, help="0 = until Ctrl+C")
    ap.add_argument("--seed", type=int, default=1)
    a = ap.parse_args()
    if a.rate / a.bots > 1.5:
        print("warning: over ~1.5 events/s per bot, ergo's fakelag will throttle", file=sys.stderr)

    rng = random.Random(a.seed)
    chans = a.channels.split(",")
    bots = await asyncio.gather(*(connect(a.host, a.port, f"loadbot{i}") for i in range(a.bots)))
    for b in bots:
        b.chans = set(rng.sample(chans, rng.randint(1, len(chans))))
        b.send("JOIN " + ",".join(sorted(b.chans)))
    readers = [asyncio.create_task(b.read_loop()) for b in bots]
    print(f"{len(bots)} bots in {', '.join(chans)} at {a.rate}/s; Ctrl+C to stop", file=sys.stderr)

    end = time.monotonic() + a.minutes * 60 if a.minutes else float("inf")
    n = 0
    start = time.monotonic()
    while time.monotonic() < end:
        act(rng, bots, chans, a.mention)
        n += 1
        for b in bots:
            await b.w.drain()
        await asyncio.sleep(max(0, start + n / a.rate - time.monotonic()))
    for b in bots:
        b.send("QUIT :load done")
        await b.w.drain()
    for t in readers:
        t.cancel()
    print(f"sent {n} events", file=sys.stderr)


if __name__ == "__main__":
    try:
        asyncio.run(main())
    except KeyboardInterrupt:
        pass
