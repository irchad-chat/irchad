#!/usr/bin/env bash
# Builds, signs and (with --upload) uploads an App Store build. See README.md › App Store.
#
#   SIGNING=<dir>   default ~/irchad-signing: dist.key, dist.pem, *.mobileprovision,
#                   asc.env (ASC_KEY_ID, ASC_ISSUER_ID) and AuthKey_<id>.p8
#
# xtool 1.24 only signs for development, so this finishes the bundle the way Xcode would:
# the compiled icon catalog (icon/, made on a Mac, see README.md), the DT* keys and the SDK
# version in the binary (App Store checks both), then rcodesign with the distribution identity.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
signing=${SIGNING:-$HOME/irchad-signing}
upload=false
[ "${1:-}" = --upload ] && upload=true

[ -f "$here/icon/Assets.car" ] || { echo "no icon/Assets.car: build it on a Mac (README.md › App Store)" >&2; exit 1; }
profile=$(ls "$signing"/*.mobileprovision | head -1)
command -v rcodesign >/dev/null || { echo "rcodesign missing: cargo install apple-codesign" >&2; exit 1; }

cd "$here"
PROFILE=release ./build-rust.sh
xtool dev build -c release
app=$here/xtool/IrChad.app
[ -d "$app" ] || { echo "no $app after xtool build" >&2; exit 1; }

sdk=$(ls -d "$HOME"/.swiftpm/swift-sdks/darwin.artifactbundle/Developer/Platforms/iPhoneOS.platform/Developer/SDKs/iPhoneOS*.sdk | sort -V | tail -1)
xcode=$HOME/.swiftpm/swift-sdks/darwin.artifactbundle/Xcode.app
find icon -maxdepth 1 -type f ! -name partial.plist -exec cp {} "$app/" \;
python3 -I "$here/finish-app.py" "$app" "$sdk" "$xcode" icon/partial.plist

cp "$profile" "$app/embedded.mobileprovision"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
# The profile's entitlements minus the wildcards Xcode never signs in.
openssl smime -inform der -verify -noverify -in "$profile" 2>/dev/null > "$work/profile.plist"
python3 -I - "$work/profile.plist" "$work/ents.plist" <<'EOF'
import plistlib, sys
e = plistlib.load(open(sys.argv[1], 'rb'))['Entitlements']
keep = {k: e[k] for k in ('application-identifier', 'com.apple.developer.team-identifier', 'beta-reports-active') if k in e}
keep['get-task-allow'] = False
plistlib.dump(keep, open(sys.argv[2], 'wb'))
EOF
# PEM, not .p12: rcodesign can't open OpenSSL 3's default PKCS#12 encryption.
rcodesign sign --pem-file "$signing/dist.key" --pem-file "$signing/dist.pem" \
    --entitlements-xml-file "IrChad:$work/ents.plist" "$app"

out=$here/xtool/IrChad.ipa
rm -rf "$work/Payload" "$out"
mkdir "$work/Payload"
cp -a "$app" "$work/Payload/"
(cd "$work" && zip -qry "$out" Payload)
echo "built $out"

if $upload; then
    # shellcheck source=/dev/null
    . "$signing/asc.env"
    # Transporter looks for AuthKey_<id>.p8 in ~/.appstoreconnect/private_keys.
    mkdir -p "$HOME/.appstoreconnect/private_keys"
    cp -n "$signing/AuthKey_$ASC_KEY_ID.p8" "$HOME/.appstoreconnect/private_keys/" || true
    iTMSTransporter -m upload -assetFile "$out" -apiKey "$ASC_KEY_ID" -apiIssuer "$ASC_ISSUER_ID" -v informational
fi
