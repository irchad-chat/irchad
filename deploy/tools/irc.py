"""Tiny blocking IRC client for dev scripts (seed, probes). Not for the app."""

import base64
import socket
import ssl
import time


class Irc:
    def __init__(self, host="127.0.0.1", port=6667, verbose=False, tls=False):
        self.sock = socket.create_connection((host, port), timeout=10)
        if tls:
            self.sock = ssl.create_default_context().wrap_socket(self.sock, server_hostname=host)
        self.buf = b""
        self.verbose = verbose
        self.log = []  # every line received, in order

    def send(self, line):
        if self.verbose:
            print(">>", line)
        self.sock.sendall(line.encode() + b"\r\n")

    def recv(self, timeout=10):
        self.sock.settimeout(timeout)
        while b"\r\n" not in self.buf:
            chunk = self.sock.recv(65536)
            if not chunk:
                raise ConnectionError("server closed connection")
            self.buf += chunk
        line, self.buf = self.buf.split(b"\r\n", 1)
        line = line.decode(errors="replace")
        if self.verbose:
            print("<<", line)
        self.log.append(line)
        if line.startswith("PING "):
            self.send("PONG " + line[5:])
        return line

    def until(self, pred, timeout=10):
        """Read lines until pred(line, cmd) is true; return all lines read."""
        deadline = time.monotonic() + timeout
        out = []
        while True:
            line = self.recv(max(0.1, deadline - time.monotonic()))
            out.append(line)
            if pred(line, command(line)):
                return out

    def drain(self, quiet=0.5):
        """Read until the server is quiet for `quiet` seconds."""
        out = []
        try:
            while True:
                out.append(self.recv(quiet))
        except (TimeoutError, socket.timeout):
            return out

    def connect(self, nick, password=None, caps=(), sasl_user=None):
        """CAP 302 + optional SASL PLAIN. Returns (cap_ls, sasl_ok)."""
        self.send("CAP LS 302")
        self.send(f"NICK {nick}")
        self.send(f"USER {nick} 0 * :{nick}")
        cap_ls = []
        while True:
            line = self.recv()
            p = params(line)
            if command(line) == "CAP" and p[1] == "LS":
                cap_ls += p[-1].split()
                if p[2] != "*":
                    break
        avail = {c.split("=", 1)[0] for c in cap_ls}
        want = [c for c in caps if c in avail]
        if password:
            want.append("sasl")
        if want:
            self.send("CAP REQ :" + " ".join(want))
            self.until(lambda l, c: c == "CAP" and params(l)[1] in ("ACK", "NAK"))
        sasl_ok = None
        if password:
            self.send("AUTHENTICATE PLAIN")
            self.until(lambda l, c: c == "AUTHENTICATE")
            user = sasl_user or nick
            # Empty authzid: ergo strips `@device` from the authcid only, and
            # rejects an authzid that differs from the (unstripped) authcid.
            blob = base64.b64encode(f"\0{user}\0{password}".encode()).decode()
            self.send("AUTHENTICATE " + blob)
            res = self.until(lambda l, c: c in ("903", "904", "905", "906", "907"))
            sasl_ok = command(res[-1]) == "903"
        self.send("CAP END")
        self.until(lambda l, c: c in ("001", "433", "ERROR", "FAIL"))
        return cap_ls, sasl_ok

    def quit(self):
        try:
            self.send("QUIT :bye")
            self.drain(0.3)
        except OSError:
            pass
        self.sock.close()


def _strip_prefix(line):
    if line.startswith("@"):
        line = line.split(" ", 1)[1]
    if line.startswith(":"):
        line = line.split(" ", 1)[1]
    return line


def command(line):
    return _strip_prefix(line).split(" ", 1)[0]


def params(line):
    """Parameters after the command, trailing included."""
    rest = _strip_prefix(line).split(" ", 1)
    if len(rest) == 1:
        return []
    rest = rest[1]
    if rest.startswith(":"):
        return [rest[1:]]
    if " :" in rest:
        head, trailing = rest.split(" :", 1)
        return head.split(" ") + [trailing]
    return rest.split(" ")
