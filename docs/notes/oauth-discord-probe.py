"""Discord OAuth2 probe: PKCE handling, /users/@me shape, rate-limit headers.

Usage: python3 -I probe.py <client_id> <secret_file>
Prints the authorize URL (also written to url.txt), waits for the callback on 127.0.0.1:8765.
Never prints the client secret or the access token.
"""
import base64, hashlib, http.server, json, secrets, sys, urllib.error, urllib.parse, urllib.request
from pathlib import Path

CLIENT_ID = sys.argv[1]
SECRET = Path(sys.argv[2]).expanduser().read_text().strip()
REDIRECT = "http://127.0.0.1:8765/callback"
UA = "irchad-oauth-probe/0.1"
HERE = Path(__file__).parent

b64 = lambda b: base64.urlsafe_b64encode(b).rstrip(b"=").decode()
RL = ("x-ratelimit-limit", "x-ratelimit-remaining", "x-ratelimit-reset-after", "x-ratelimit-bucket",
      "x-ratelimit-scope", "x-ratelimit-global", "retry-after")


def pkce():
    v = b64(secrets.token_bytes(32))
    return v, b64(hashlib.sha256(v.encode()).digest())


def authorize_url(state, challenge):
    q = {"client_id": CLIENT_ID, "response_type": "code", "redirect_uri": REDIRECT,
         "scope": "identify email", "state": state, "prompt": "consent"}
    if challenge:
        q |= {"code_challenge": challenge, "code_challenge_method": "S256"}
    return "https://discord.com/oauth2/authorize?" + urllib.parse.urlencode(q)


def wait_for_code(state, url):
    (HERE / "url.txt").write_text(url + "\n")
    print("OPEN:", url, flush=True)
    got = {}

    class H(http.server.BaseHTTPRequestHandler):
        def do_GET(self):
            q = urllib.parse.parse_qs(urllib.parse.urlsplit(self.path).query)
            if not self.path.startswith("/callback"):
                self.send_response(404); self.end_headers(); return
            got.update({k: v[0] for k, v in q.items()})
            self.send_response(200); self.send_header("content-type", "text/plain"); self.end_headers()
            self.wfile.write(b"probe got the callback; you can close this tab")

        def log_message(self, *a):
            pass

    srv = http.server.HTTPServer(("127.0.0.1", 8765), H)
    while "code" not in got and "error" not in got:
        srv.handle_request()
    srv.server_close()
    if "error" in got:
        sys.exit(f"authorize error: {got}")
    assert got.get("state") == state, "state mismatch"
    return got["code"]


def call(method, url, form=None, token=None):
    data = urllib.parse.urlencode(form).encode() if form is not None else None
    req = urllib.request.Request(url, data, method=method, headers={"User-Agent": UA})
    if token:
        req.add_header("Authorization", f"Bearer {token}")
    else:
        req.add_header("Authorization", "Basic " + base64.b64encode(f"{CLIENT_ID}:{SECRET}".encode()).decode())
    if data is not None:
        req.add_header("Content-Type", "application/x-www-form-urlencoded")
    try:
        with urllib.request.urlopen(req, timeout=15) as r:
            status, headers, body = r.status, r.headers, r.read()
    except urllib.error.HTTPError as e:
        status, headers, body = e.code, e.headers, e.read()
    rl = {h: headers[h] for h in RL if headers.get(h)}
    try:
        body = json.loads(body)
    except ValueError:
        body = {"_raw": body[:200].decode(errors="replace")}
    return status, rl, body


def exchange(code, verifier):
    form = {"grant_type": "authorization_code", "code": code, "redirect_uri": REDIRECT}
    if verifier is not None:
        form["code_verifier"] = verifier
    return call("POST", "https://discord.com/api/v10/oauth2/token", form)


def show(label, status, rl, body):
    safe = {k: ("<redacted>" if "token" in k else v) for k, v in body.items()}
    print(f"{label}: HTTP {status} {safe}\n    ratelimit: {rl}", flush=True)


report = {}

# Round 1: authorize with a challenge, exchange with a WRONG verifier first, then the right one.
verifier, challenge = pkce()
state = secrets.token_urlsafe(16)
code = wait_for_code(state, authorize_url(state, challenge))
s1, rl, b1 = exchange(code, pkce()[0]); show("exchange wrong verifier", s1, rl, b1)
token = b1.get("access_token")
if token:
    report["pkce"] = "IGNORED: wrong verifier accepted"
else:
    s2, rl, b2 = exchange(code, verifier); show("exchange right verifier (same code)", s2, rl, b2)
    token = b2.get("access_token")
    if token:
        report["pkce"] = "ENFORCED: wrong verifier refused, right one accepted, code survived a failed try"
    else:
        report["pkce"] = "wrong verifier refused; code burned by failed try -> round 2 to confirm"

# Round 2 (only if ambiguous): fresh code, right verifier only.
if not token:
    verifier, challenge = pkce()
    state = secrets.token_urlsafe(16)
    code = wait_for_code(state, authorize_url(state, challenge))
    s3, rl, b3 = exchange(code, verifier); show("round 2 right verifier", s3, rl, b3)
    token = b3.get("access_token")
    report["pkce"] += "; round 2 " + ("ENFORCED (right verifier accepted)" if token else "FAILED")

if token:
    s, rl, me = call("GET", "https://discord.com/api/v10/users/@me", token=token)
    print(f"/users/@me: HTTP {s}\n    ratelimit: {rl}")
    # Keys and value shapes only, no personal values.
    print("    fields:", {k: type(v).__name__ for k, v in me.items()})
    report["username_charset_ok"] = all(c in "abcdefghijklmnopqrstuvwxyz0123456789_." for c in me.get("username", ""))
    report["username_has_dot"] = "." in me.get("username", "")
    report["email_verified"] = me.get("verified")
    s, rl, b = call("POST", "https://discord.com/api/v10/oauth2/token/revoke",
                    {"token": token, "token_type_hint": "access_token"})
    print(f"revoke: HTTP {s}")

print("\nREPORT", json.dumps(report, indent=1))
