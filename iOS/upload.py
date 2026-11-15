"""Upload an .ipa through App Store Connect's Build Uploads API.

Transporter on Linux can't analyse the app ("Export an AppStoreInfo.plist from Xcode"),
and the API doesn't need that. The flow follows xtool PR #327: create a buildUpload,
reserve its file, PUT the parts, mark it uploaded, then poll until Apple accepts or rejects it.

usage: upload.py <ipa> <key id> <issuer id> <key .p8>
"""
import base64
import json
import os
import plistlib
import sys
import time
import urllib.error
import urllib.request
import zipfile

from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.asymmetric import ec
from cryptography.hazmat.primitives.asymmetric.utils import decode_dss_signature

API = "https://api.appstoreconnect.apple.com"


def b64(data):
    return base64.urlsafe_b64encode(data).rstrip(b"=").decode()


def token(key_id, issuer, key_path):
    key = serialization.load_pem_private_key(open(key_path, "rb").read(), None)
    now = int(time.time())
    head = b64(json.dumps({"alg": "ES256", "kid": key_id, "typ": "JWT"}).encode())
    body = b64(json.dumps({"iss": issuer, "iat": now, "exp": now + 1200, "aud": "appstoreconnect-v1"}).encode())
    r, s = decode_dss_signature(key.sign(f"{head}.{body}".encode(), ec.ECDSA(hashes.SHA256())))
    # JWT wants raw r||s, not DER.
    return f"{head}.{body}.{b64(r.to_bytes(32, 'big') + s.to_bytes(32, 'big'))}"


class Api:
    def __init__(self, key_id, issuer, key_path):
        self.creds = (key_id, issuer, key_path)
        self.fresh = 0

    def call(self, method, path, body=None):
        if time.time() - self.fresh > 600:
            self.jwt, self.fresh = token(*self.creds), time.time()
        req = urllib.request.Request(API + path, method=method, data=json.dumps(body).encode() if body else None)
        req.add_header("Authorization", f"Bearer {self.jwt}")
        req.add_header("Content-Type", "application/json")
        try:
            with urllib.request.urlopen(req, timeout=60) as resp:
                raw = resp.read()
                return json.loads(raw) if raw else {}
        except urllib.error.HTTPError as e:
            sys.exit(f"{method} {path}: HTTP {e.code}\n{e.read().decode(errors='replace')}")


def bundle_info(ipa):
    with zipfile.ZipFile(ipa) as z:
        name = next(n for n in z.namelist() if n.count("/") == 2 and n.endswith(".app/Info.plist"))
        return plistlib.loads(z.read(name))


def main():
    ipa, key_id, issuer, key_path = sys.argv[1:5]
    info = bundle_info(ipa)
    api = Api(key_id, issuer, key_path)

    bundle = info["CFBundleIdentifier"]
    apps = api.call("GET", f"/v1/apps?filter%5BbundleId%5D={bundle}&fields%5Bapps%5D=bundleId")["data"]
    app = next((a["id"] for a in apps if a["attributes"]["bundleId"] == bundle), None)
    if not app:
        sys.exit(f"no App Store Connect app for {bundle}")

    upload = api.call("POST", "/v1/buildUploads", {"data": {
        "type": "buildUploads",
        "attributes": {
            "cfBundleShortVersionString": info["CFBundleShortVersionString"],
            "cfBundleVersion": info["CFBundleVersion"],
            "platform": "IOS",
        },
        "relationships": {"app": {"data": {"type": "apps", "id": app}}},
    }})["data"]["id"]

    size = os.path.getsize(ipa)
    file = api.call("POST", "/v1/buildUploadFiles", {"data": {
        "type": "buildUploadFiles",
        "attributes": {"assetType": "ASSET", "fileName": os.path.basename(ipa), "fileSize": size, "uti": "com.apple.ipa"},
        "relationships": {"buildUpload": {"data": {"type": "buildUploads", "id": upload}}},
    }})["data"]

    with open(ipa, "rb") as f:
        for op in file["attributes"]["uploadOperations"]:
            f.seek(op["offset"])
            req = urllib.request.Request(op["url"], method=op.get("method") or "PUT", data=f.read(op["length"]))
            for h in op.get("requestHeaders") or []:
                req.add_header(h["name"], h["value"])
            urllib.request.urlopen(req, timeout=300).read()
            print(f"sent {min(op['offset'] + op['length'], size)}/{size} bytes")

    api.call("PATCH", f"/v1/buildUploadFiles/{file['id']}", {"data": {
        "type": "buildUploadFiles", "id": file["id"], "attributes": {"uploaded": True},
    }})

    wait, deadline = 5, time.time() + 1800
    while time.time() < deadline:
        state = api.call("GET", f"/v1/buildUploads/{upload}")["data"]["attributes"].get("state") or {}
        for kind in ("errors", "warnings", "infos"):
            for m in state.get(kind) or []:
                print(f"{kind[:-1]}: {m.get('code')}: {m.get('description')}")
        if state.get("state") == "COMPLETE":
            print("uploaded; Apple is processing it for TestFlight")
            return
        if state.get("state") == "FAILED":
            sys.exit("upload rejected")
        print(f"state: {state.get('state')}")
        time.sleep(wait)
        wait = min(wait * 1.5, 30)
    sys.exit("gave up waiting after 30 minutes; check App Store Connect")


if __name__ == "__main__":
    main()
