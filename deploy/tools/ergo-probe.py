#!/usr/bin/env python3
"""Phase 0 ergo capability probe. Prints a transcript-style report.

    python3 deploy/seed.py && python3 deploy/tools/ergo-probe.py

Needs the dev stack (raw IRC on 127.0.0.1:6667) and the seeded accounts.
Findings are written up in docs/notes/ergo-caps.md.
"""

import base64
import hashlib
import hmac
import os
import secrets
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from irc import Irc, command, params  # noqa: E402

CAPS = [
    "message-tags", "server-time", "batch", "labeled-response", "echo-message",
    "draft/chathistory", "draft/event-playback", "draft/metadata-2", "draft/read-marker",
    "draft/message-redaction", "draft/multiline", "account-tag", "extended-join",
    "away-notify", "account-notify", "multi-prefix", "userhost-in-names", "cap-notify",
    "setname", "standard-replies", "no-implicit-names",
]


def section(title):
    print(f"\n## {title}\n")


def show(lines, pred=lambda l: True, limit=60):
    shown = [l for l in lines if pred(l)]
    print("```")
    for l in shown[:limit]:
        print(l if len(l) < 400 else l[:400] + f"… ({len(l)} bytes)")
    if len(shown) > limit:
        print(f"… {len(shown) - limit} more")
    print("```")


def scram_sha256(c, user, password):
    """SASL SCRAM-SHA-256 (RFC 5802/7677). Returns True on 903."""
    c.send("AUTHENTICATE SCRAM-SHA-256")
    c.until(lambda l, cmd: cmd == "AUTHENTICATE")
    nonce = base64.b64encode(secrets.token_bytes(18)).decode()
    bare = f"n={user.replace('=', '=3D').replace(',', '=2C')},r={nonce}"
    c.send("AUTHENTICATE " + base64.b64encode(("n,," + bare).encode()).decode())
    res = c.until(lambda l, cmd: cmd in ("AUTHENTICATE", "904"))
    if command(res[-1]) != "AUTHENTICATE":
        return False
    server_first = base64.b64decode(params(res[-1])[0]).decode()
    attrs = dict(kv.split("=", 1) for kv in server_first.split(","))
    salt, iters, rnonce = base64.b64decode(attrs["s"]), int(attrs["i"]), attrs["r"]
    salted = hashlib.pbkdf2_hmac("sha256", password.encode(), salt, iters)
    client_key = hmac.new(salted, b"Client Key", "sha256").digest()
    stored = hashlib.sha256(client_key).digest()
    without_proof = f"c=biws,r={rnonce}"
    auth_msg = f"{bare},{server_first},{without_proof}".encode()
    sig = hmac.new(stored, auth_msg, "sha256").digest()
    proof = base64.b64encode(bytes(a ^ b for a, b in zip(client_key, sig))).decode()
    final = f"{without_proof},p={proof}"
    c.send("AUTHENTICATE " + base64.b64encode(final.encode()).decode())
    res = c.until(lambda l, cmd: cmd in ("AUTHENTICATE", "903", "904"))
    if command(res[-1]) == "AUTHENTICATE":
        c.send("AUTHENTICATE +")
        res = c.until(lambda l, cmd: cmd in ("903", "904"))
    return command(res[-1]) == "903"


def connect_scram(nick, user, password, caps=CAPS):
    c = Irc()
    c.send("CAP LS 302")
    c.send(f"NICK {nick}")
    c.send(f"USER {nick} 0 * :{nick}")
    c.until(lambda l, cmd: cmd == "CAP" and params(l)[1] == "LS" and params(l)[2] != "*")
    c.send("CAP REQ :" + " ".join(list(caps) + ["sasl"]))
    c.until(lambda l, cmd: cmd == "CAP" and params(l)[1] in ("ACK", "NAK"))
    ok = scram_sha256(c, user, password)
    c.send("CAP END")
    c.until(lambda l, cmd: cmd in ("001", "433", "ERROR"))
    return c, ok


def main():
    # --- CAP LS + ISUPPORT -------------------------------------------------
    section("CAP LS 302")
    c = Irc()
    cap_ls, ok = c.connect("alice", "alicepass", caps=CAPS, sasl_user="alice@probe")
    print("```")
    for cap in sorted(cap_ls):
        print(cap)
    print("```")
    lines = c.drain(1)
    section("ISUPPORT (005)")
    show(c.log, lambda l: command(l) == "005")
    print(f"\nSASL PLAIN as `alice@probe`: {'ok' if ok else 'FAILED'}")

    # --- metadata limits ------------------------------------------------------
    section("Metadata limits")
    results = []
    for key, size in [("chad/probe", 300), ("chad/probe", 340), ("chad/probe", 341), ("chad/probe", 400)]:
        c.send(f"METADATA * SET {key} :{'x' * size}")
        res = c.until(lambda l, cmd: cmd in ("761", "FAIL") or cmd.startswith("76"))
        results.append(f"SET {key} value={size}B (key+value={len(key) + size}B) -> {res[-1][:120]}")
    c.send("METADATA * SET Chad/Bad :x")
    res = c.until(lambda l, cmd: cmd in ("761", "FAIL"))
    results.append(f"SET Chad/Bad (uppercase key) -> {res[-1][:120]}")
    c.send("METADATA * SET chad/probe")
    c.drain(0.5)
    # Rate limit: default client-throttle is 10 per 2m.
    sent = 0
    limited = None
    for i in range(15):
        c.send(f"METADATA * SET chad/rl :{i}")
        res = c.until(lambda l, cmd: cmd in ("761", "FAIL"))
        sent += 1
        if command(res[-1]) == "FAIL":
            limited = res[-1]
            break
    results.append(f"rate limit: {sent} SETs before -> {limited}")
    c.send("METADATA * SET chad/rl")
    c.drain(0.5)
    print("```")
    print("\n".join(results))
    print("```")
    c.quit()

    # --- client tags through chathistory -------------------------------------
    section("Client-only tags on PRIVMSG through CHATHISTORY")
    big = base64.b64encode(secrets.token_bytes(2400)).decode()  # 3200 chars, ~Olm prekey msg size
    small = "AwogQUJD"
    a = Irc()
    a.connect("alice", "alicepass", caps=CAPS, sasl_user="alice@probe")
    a.drain(1)
    a.send(f"@+chad/e2e={small} PRIVMSG #dev :[probe small tag]")
    a.send(f"@+chad/e2e={big} PRIVMSG #dev :[probe big tag]")
    a.send(f"@+chad/e2e={small};+draft/reply=fake PRIVMSG bob :[probe dm tag]")
    sent = a.drain(1)
    echoes = [l for l in sent if command(l) == "PRIVMSG"]
    print(f"echo-message lines: {len(echoes)}; tag present on echoes: "
          f"{[('+chad/e2e=' in l) for l in echoes]}")
    errs = [l for l in sent if command(l) in ("FAIL", "417", "NOTICE") and "probe" not in l]
    if errs:
        show(errs)
    a.send("CHATHISTORY LATEST #dev * 2")
    hist = a.drain(1)
    print("CHATHISTORY LATEST #dev * 2:")
    show(hist, lambda l: command(l) in ("PRIVMSG", "BATCH"))
    big_ok = any(f"+chad/e2e={big}" in l for l in hist)
    print(f"big tag ({len(big)} chars) survives replay intact: {big_ok}")
    a.send("CHATHISTORY LATEST bob * 1")
    hist = a.drain(1)
    print("CHATHISTORY LATEST bob * 1:")
    show(hist, lambda l: command(l) in ("PRIVMSG", "BATCH"))
    over = "A" * 4100
    a.send(f"@+chad/e2e={over} PRIVMSG #dev :[probe oversize tag]")
    res = a.drain(1)
    print("oversize (4100-char) tag:")
    show(res, lambda l: True, limit=5)
    a.quit()

    # --- SASL device IDs ---------------------------------------------------
    section("SASL usernames with device IDs")
    for mech, user in [("PLAIN", "bob@laptop"), ("SCRAM-SHA-256", "bob@laptop"), ("SCRAM-SHA-256", "bob")]:
        if mech == "PLAIN":
            b = Irc()
            _, ok = b.connect("bob", "bobpass", sasl_user=user)
        else:
            b, ok = connect_scram("bob", user, "bobpass")
        print(f"- {mech} as `{user}`: {'ok' if ok else 'FAILED'}")
        b.quit()

    # --- always-on attach + offline DMs ------------------------------------
    section("Always-on: offline DMs and attach burst")
    # Make sure bob has no connected sessions, then DM him from a fresh nick... alice.
    time.sleep(1)
    before = time.strftime("%Y-%m-%dT%H:%M:%S.000Z", time.gmtime(time.time() - 1))
    a = Irc()
    a.connect("alice", "alicepass", caps=CAPS, sasl_user="alice@probe")
    a.drain(0.5)
    marker = secrets.token_hex(4)
    a.send(f"PRIVMSG bob :offline dm {marker}")
    a.send(f"PRIVMSG #general :channel msg while bob offline {marker}")
    a.drain(0.5)
    a.send("WHOIS bob")
    whois = a.drain(0.5)
    print("WHOIS bob while bob has no sessions (away = auto-away working):")
    show(whois, lambda l: command(l) in ("301", "311", "330", "671", "320", "318"))
    a.quit()

    b, ok = connect_scram("bob", "bob@laptop", "bobpass")
    burst = b.drain(2)
    print(f"bob@laptop reattach (SCRAM ok={ok}); burst after 001, counts by command:")
    counts = {}
    for l in b.log:
        counts[command(l)] = counts.get(command(l), 0) + 1
    print("```")
    print(", ".join(f"{k}={v}" for k, v in sorted(counts.items())))
    print("```")
    show(b.log, lambda l: command(l) in ("JOIN", "332", "333", "353", "366", "PRIVMSG", "NOTICE", "AWAY", "BATCH", "MARKREAD"), limit=40)
    b.send(f"CHATHISTORY TARGETS timestamp={before} timestamp=2099-01-01T00:00:00.000Z 50")
    targets = b.drain(1)
    print(f"CHATHISTORY TARGETS since {before}:")
    show(targets, lambda l: command(l) in ("CHATHISTORY", "BATCH", "FAIL"))
    b.quit()


if __name__ == "__main__":
    main()
