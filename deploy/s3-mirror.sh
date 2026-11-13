#!/bin/sh
# Copy a Gitea release's assets to the downloads.gentoo.party garage bucket,
# once per prefix: `s3-mirror.sh <release id> irchad/latest irchad/v0.1.3`.
# Env: API (repo API URL), TOKEN (Gitea token), S3_ACCESS_KEY, S3_SECRET_KEY.
set -eu
id=$1; shift
s3=${S3_ENDPOINT:-http://192.0.2.10:3900}/downloads.gentoo.party
[ -n "$S3_ACCESS_KEY" ] && [ -n "$S3_SECRET_KEY" ] || { echo "S3_ACCESS_KEY/S3_SECRET_KEY secrets missing"; exit 1; }
tmp=$(mktemp)
trap 'rm -f "$tmp"' EXIT
# Not piped straight in: `set -e` wouldn't see a failed listing.
assets=$(curl -fsS ${TOKEN:+-H "Authorization: token $TOKEN"} "$API/releases/$id/assets" |
    jq -er '.[] | "\(.name)\t\(.browser_download_url)"')
[ -n "$assets" ] || { echo "release $id has no assets"; exit 1; }
echo "$assets" | while IFS="$(printf '\t')" read -r name url; do
        curl -fsSL ${TOKEN:+-H "Authorization: token $TOKEN"} -o "$tmp" "$url"
        for prefix in "$@"; do
            echo "$prefix/$name"
            # Short max-age: latest/ and nightly/ are overwritten in place behind Cloudflare.
            curl -fsS --aws-sigv4 aws:amz:garage:s3 --user "$S3_ACCESS_KEY:$S3_SECRET_KEY" \
                -H "x-amz-content-sha256: UNSIGNED-PAYLOAD" -H "Content-Type: application/octet-stream" \
                -H "Cache-Control: max-age=300" -T "$tmp" "$s3/$prefix/$name"
        done
    done
