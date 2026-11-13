#!/usr/bin/env bash
# Profile irchad under real use: ./scripts/profile.sh [irchad args]
# Use the app normally, quit it, then hand target/profiling/report/*.txt to whoever is optimising.
# Repeatable load: deploy/tools/chatter.py against the dev stack, with IrChad in #load1..3.
# Works unprivileged at kernel.perf_event_paranoid=2 (user-space samples only).
set -euo pipefail
cd "$(dirname "$0")/.."

# Frame pointers keep perf's unwinding cheap enough for hour-long sessions (dwarf would be GBs).
RUSTFLAGS="${RUSTFLAGS:-} -C force-frame-pointers=yes" cargo build -q --profile profiling -p chad-desktop
data=target/profiling/perf.data out=target/profiling/report
rm -rf "$data" "$out" && mkdir -p "$out"
# PROFILE_WRAP lets a test bound the run, e.g. PROFILE_WRAP="timeout -s INT 10".
perf record -q -e cpu-clock:u -F 499 --call-graph fp --user-callchains -o "$data" -- ${PROFILE_WRAP:-} target/profiling/irchad "$@" || true
[ -s "$data" ] || { echo "perf recorded nothing" >&2; exit 1; }

r() { perf report -i "$data" --stdio --quiet "$@" 2>/dev/null; }
r --no-children --sort comm -g none > "$out/threads.txt"
r --no-children --sort comm,dso,sym -g none --percent-limit 0.3 > "$out/self.txt"
r --children --sort comm,sym -g folded,0.5,caller --percent-limit 2 > "$out/inclusive.txt"
{ echo "irchad $(git describe --always --dirty), $(date -Is)"; perf report -i "$data" --header-only 2>/dev/null | grep -E 'duration|cpudesc|nrcpus|sample'; } > "$out/meta.txt"
wc -c "$out"/*.txt
