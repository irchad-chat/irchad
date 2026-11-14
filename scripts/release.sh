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

# Release notes: Claude drafts them from the commits since the last tag, you
# edit them, and they become the tag message that release.yml publishes.
notes=$(mktemp --suffix=.md); trap 'rm -f "$notes"' EXIT
prev=$(git describe --tags --abbrev=0 2>/dev/null) && range="$prev..HEAD" || range=HEAD
log=$(git log --no-merges --invert-grep --grep='^release ' --format='- %s%n%w(0,2,2)%b' "$range")
{ echo "IrChad $v"; echo
  if command -v claude >/dev/null; then
    echo "drafting notes with claude…" >&2
    # prompt on stdin: --tools is variadic and would eat a positional prompt
    printf '%s\n\n%s\n' "Write release notes for IrChad $v, a Discord-like desktop/mobile IRC client, from these commits since ${prev:-the start}.
Markdown bullets only, no headings, no preamble. One short user-facing line each; merge related commits; drop CI, refactor, docs and other internal-only changes." "$log" |
      claude -p --tools "" || echo "$log"
  else echo "$log"; fi
} >"$notes"
"${EDITOR:-vi}" "$notes"
grep -q '[^[:space:]]' <(tail -n +3 "$notes") || { echo "empty release notes, aborting" >&2; exit 1; }

sed -i "0,/^version = \".*\"/s//version = \"$v\"/" Cargo.toml
sed -i "s|<release version=\"[^\"]*\" date=\"[^\"]*\"/>|<release version=\"$v\" date=\"$(date +%F)\"/>|" \
  packaging/linux/party.gentoo.IrChad.metainfo.xml
cargo update -q -w  # rewrites only the workspace crates' versions in Cargo.lock

git commit -qam "release $v"
git tag -a "$tag" --cleanup=verbatim -F "$notes"
git push -q origin main "$tag"
echo "pushed $tag; release.yml builds it"
