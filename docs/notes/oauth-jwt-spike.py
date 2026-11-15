"""Spike: ergo jwt-auth + IRCV3BEARER + device ID via USER. Prints PASS/FAIL per check. See oauth.md.

Run in a scratch dir holding priv.pem/pub.pem (openssl genpkey -algorithm ed25519) and an ircd.yaml
copied from deploy/ergo.yaml with jwt-auth enabled as in oauth.md, against a throwaway ergo:
  podman run -d --name jwt-spike --init -p 127.0.0.1:6697:6667 -p 127.0.0.1:8090:8089 \
    -v $PWD/ircd.yaml:/ircd/ircd.yaml:ro,Z -v $PWD/pub.pem:/ircd/jwt_pub.pem:ro,Z ghcr.io/ergochat/ergo:v2.19.1
  python3 oauth-jwt-spike.py
"""
import base64, json, socket, time, urllib.request, hmac, hashlib
from cryptography.hazmat.primitives import serialization
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey

IRC = ("127.0.0.1", 6697)
API = "http://127.0.0.1:8090"
KEY = serialization.load_pem_private_key(open("priv.pem", "rb").read(), None)
OTHER = Ed25519PrivateKey.generate()
b64 = lambda b: base64.urlsafe_b64encode(b).rstrip(b"=").decode()


def jwt(claims, key=KEY, alg="EdDSA"):
    h = b64(json.dumps({"alg": alg, "typ": "JWT"}).encode())
    p = b64(json.dumps(claims).encode())
    msg = f"{h}.{p}".encode()
    sig = hmac.new(key, msg, hashlib.sha256).digest() if alg == "HS256" else key.sign(msg)
    return f"{h}.{p}.{b64(sig)}"


def claims(account="bob", **kw):
    c = {"account": account, "aud": "irchad-ergo", "exp": int(time.time()) + 60}
    c.update(kw)
    return {k: v for k, v in c.items() if v is not None}


def api(path, body):
    req = urllib.request.Request(API + path, json.dumps(body).encode(),
                                 {"Authorization": "Bearer chad-dev-api-token"})
    return json.load(urllib.request.urlopen(req))


class Conn:
    def __init__(s):
        s.sock = socket.create_connection(IRC, timeout=5)
        s.buf = b""

    def send(s, line):
        s.sock.sendall(line.encode() + b"\r\n")

    def until(s, *codes, timeout=5):
        """Read lines until one whose command is in codes; return all lines read."""
        out, end = [], time.time() + timeout
        while time.time() < end:
            while b"\r\n" in s.buf:
                line, s.buf = s.buf.split(b"\r\n", 1)
                line = line.decode(errors="replace")
                out.append(line)
                if line.startswith("PING"):
                    s.send("PONG" + line[4:])
                parts = line.split(" ")
                cmd = parts[1] if line.startswith(":") or line.startswith("@") else parts[0]
                if line.startswith("@"):
                    cmd = parts[2]
                if cmd in codes:
                    return out
            try:
                s.buf += s.sock.recv(4096)
            except socket.timeout:
                break
        return out

    def close(s):
        s.send("QUIT")
        s.sock.close()


def login(token, user="bob@laptop", nick="bob", ttype="jwt"):
    """Returns (ok, conn, lines). Pre-registration SASL IRCV3BEARER."""
    c = Conn()
    c.send("CAP LS 302")
    c.send(f"NICK {nick}")
    c.send(f"USER {user} 0 * :spike")
    ls = c.until("CAP")
    c.send("CAP REQ :sasl")
    c.until("CAP")
    c.send("AUTHENTICATE IRCV3BEARER")
    c.until("AUTHENTICATE")
    blob = base64.b64encode(f"\0{ttype}\0{token}".encode()).decode()
    assert len(blob) < 400, "would need 400-byte chunking"
    c.send(f"AUTHENTICATE {blob}")
    lines = c.until("903", "904", "902", "905")
    ok = any(" 903 " in l for l in lines)
    c.send("CAP END")
    lines += c.until("001", "ERROR", "433", "903")
    return ok, c, ls + lines


results = []
def check(name, cond, detail=""):
    results.append(cond)
    print(("PASS " if cond else "FAIL ") + name + (f"  [{detail}]" if detail and not cond else ""))


# 0. advertised?
c = Conn(); c.send("CAP LS 302"); ls = []
while not ls or " LS * " in ls[-1]: ls += c.until("CAP")
c.sock.close()
check("sasl cap advertises IRCV3BEARER", any("IRCV3BEARER" in l for l in ls), ls)

# 1. autocreate off: unknown account is refused
ok, c, lines = login(jwt(claims("bob")))
check("unknown account refused (autocreate: false)", not ok, lines); c.sock.close()

# 2. chadd's path: create account through the API, passwordless
r = api("/v1/ns/saregister", {"accountName": "bob", "passphrase": ""})
check("saregister with empty passphrase", r.get("success"), r)
info = api("/v1/ns/info", {"accountName": "bob"})
check("ns/info returns registeredAt", bool(info.get("registeredAt")), info)
print("     registeredAt:", info.get("registeredAt"))

# 3. JWT login with device ID from USER
ok, laptop, lines = login(jwt(claims("bob")), user="bob@laptop")
check("JWT login succeeds", ok, lines)
check("registered as bob", any(" 001 bob " in l for l in lines), lines)

# 4. second device
ok, phone, lines = login(jwt(claims("bob")), user="bob@phone", nick="bob")
check("second device JWT login", ok, lines)

laptop.send("NS CLIENTS LIST")
time.sleep(1)
out = "\n".join(laptop.until("NEVER", timeout=2))
devs = [l.split("Device ID:")[-1].strip() for l in out.splitlines() if "Device ID:" in l]
check("NS CLIENTS LIST shows both device IDs", sorted(devs) == ["laptop", "phone"], out)

# 5. always-on: disconnect both, account's client stays
laptop.close(); phone.close(); time.sleep(1)
w = Conn(); w.send("NICK watcher"); w.send("USER w 0 * :w")
w.until("001"); w.send("WHOIS bob"); out = w.until("318")
check("bob stays present after disconnect (always-on)", any(" 311 " in l for l in out), out)
w.close()

# 6. negative cases
now = int(time.time())
neg = {
    "expired": jwt(claims(exp=now - 10)),
    "no exp": jwt(claims(exp=None)),
    "wrong aud": jwt(claims(aud="chadd-session")),
    "no aud": jwt(claims(aud=None)),
    "wrong key": jwt(claims(), key=OTHER),
    "alg confusion HS256 w/ pubkey": jwt(claims(), key=open("pub.pem", "rb").read(), alg="HS256"),
    "alg none": jwt(claims()).rsplit(".", 1)[0].replace(b64(b'{"alg": "EdDSA", "typ": "JWT"}'), b64(b'{"alg":"none"}')) + ".",
    "oauth2 type (disabled)": jwt(claims()),
}
for name, tok in neg.items():
    ok, c, lines = login(tok, ttype="oauth2" if "oauth2" in name else "jwt")
    check(f"rejects {name}", not ok, lines); c.sock.close()

# 7. replay: same token twice within exp
tok = jwt(claims())
ok1, c1, _ = login(tok, user="bob@a"); c1.close()
ok2, c2, _ = login(tok, user="bob@b"); c2.close()
print(f"INFO token replay within exp accepted: {ok1 and ok2} (expected True: no jti check)")

# 8. passwordless account: user can't set first password; chadd can via API
ok, c, _ = login(jwt(claims()), user="bob@laptop")
c.send("NS PASSWD * hunter22 hunter22"); out = c.until("NEVER", timeout=2); c.close()
check("NS PASSWD * refused for credential-less account (externally managed)",
      any("managed externally" in l for l in out), out)
r = api("/v1/ns/passwd", {"accountName": "bob", "passphrase": "hunter22"})
check("API /v1/ns/passwd sets first password", r.get("success"), r)
r = api("/v1/check_auth", {"accountName": "bob", "passphrase": "hunter22"})
check("check_auth works after that (FILEHOST Basic, plain clients)", r.get("success"), r)
ok, c, _ = login(jwt(claims()), user="bob@laptop"); c.close()
check("JWT login still works once a password exists", ok)
time.sleep(0.5)
ok, c, _ = login(jwt(claims()), user="bob@laptop")
c.send("NS PASSWD hunter22 newpass1 newpass1"); out = c.until("NEVER", timeout=2); c.close()
r = api("/v1/check_auth", {"accountName": "bob", "passphrase": "newpass1"})
check("user can change it themselves after that (NS PASSWD)", r.get("success"), out)

# 9. recycled name: unregister bob, re-register -> registeredAt changes
r = api("/v1/ns/info", {"accountName": "bob"}); before = r.get("registeredAt")
ok, c, _ = login(jwt(claims()), user="bob@laptop")
c.send("NS UNREGISTER bob"); out = c.until("NEVER", timeout=2)
code = next((l.split("NS UNREGISTER bob ")[1].split()[0].strip("\x02") for l in out if "NS UNREGISTER bob " in l), None)
c.send(f"NS UNREGISTER bob {code}"); c.until("NEVER", timeout=2); c.sock.close()
ok, c, _ = login(jwt(claims())); c.sock.close()
check("JWT for unregistered account refused", not ok)
time.sleep(1.1)
api("/v1/ns/saregister", {"accountName": "bob", "passphrase": "someoneelse"})
after = api("/v1/ns/info", {"accountName": "bob"}).get("registeredAt")
check("re-registered name has a different registeredAt", before != after, (before, after))

print(f"\n{sum(results)}/{len(results)} checks passed")
