#!/usr/bin/env bash
# 06-tail-logs.sh — show both log surfaces.
#
# Tail mode  (default): one-shot GET /logs/tail?lines=N. Cheap, scriptable.
# Follow mode (--follow): live SSE stream from /logs/stream. Ctrl-C exits.
#
# The recipe runs both back-to-back so you see the wire shape and the
# realtime experience.

set -euo pipefail
cd "$(dirname "$0")/.."

cyan() { printf '\033[36m%s\033[0m\n' "$*"; }
dim()  { printf '\033[2m%s\033[0m\n' "$*"; }

cyan "1. one-shot tail of the last 30 validator stdout lines:"
echo "$ k256-replay logs -n 30"
echo
k256-replay logs -n 30 | tail -30
echo

cyan "2. live SSE follow (5 s sample, then Ctrl-C):"
echo "$ k256-replay logs --follow"
dim "   (this script terminates the follow after 5 seconds — when you"
dim "    run it yourself, just hit Ctrl-C to leave at any time.)"
echo
k256-replay logs --follow &
pid=$!
sleep 5
kill -INT "$pid" 2>/dev/null || true
wait "$pid" 2>/dev/null || true

echo
cyan "Tip: stream into grep to watch for one substring (e.g. a program log)."
cat <<'EOF'
  k256-replay logs --follow | grep --line-buffered 'Program JUP'

The CLI swallows SSE keepalive comments (`:` heartbeats every 15 s) so
the stream is clean stdout. The orchestrator buffers ~5 k lines so a
tail-then-follow gives you continuity across a brief disconnect.
EOF
