#!/usr/bin/env bash
# Bump, commit, tag and push a release: ./scripts/release.sh vX.Y.Z
# Run docs/release.md's checklist (fuzz, live checks, by hand) first.
set -euo pipefail
cd "$(dirname "$0")/.."

tag=${1:?usage: $0 vX.Y.Z}
[[ $tag =~ ^v([0-9]+\.[0-9]+\.[0-9]+)$ ]] || { echo "bad tag: $tag (want vX.Y.Z)" >&2; exit 1; }
v=${BASH_REMATCH[1]}
[ "$(git branch --show-current)" = main ] || { echo "not on main" >&2; exit 1; }
[ -z "$(git status --porcelain)" ] || { echo "working tree not clean" >&2; exit 1; }
git rev-parse -q --verify "refs/tags/$tag" >/dev/null && { echo "$tag already exists" >&2; exit 1; }

sed -i "0,/^version = \".*\"/s//version = \"$v\"/" Cargo.toml
sed -i "s|<release version=\"[^\"]*\" date=\"[^\"]*\"/>|<release version=\"$v\" date=\"$(date +%F)\"/>|" \
  packaging/linux/party.gentoo.IrChad.metainfo.xml
cargo update -q -w  # rewrites only the workspace crates' versions in Cargo.lock

git commit -qam "release $v"
git tag -a "$tag" -m "IrChad $v"
git push -q origin main "$tag"
echo "pushed $tag; release.yml builds it"
