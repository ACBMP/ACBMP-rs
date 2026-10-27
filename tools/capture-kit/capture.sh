#!/usr/bin/env bash
# Retail movement capture: ./capture.sh MOVE [MOVE...]
# For each move name: waits for Enter, records until Enter again, saves <move>-<n>.csv.
# The game (ACBMP.exe under Wine) must be running and in a match/level.
set -u
here=$(cd "$(dirname "$0")" && pwd)
bin="$here/acb-trace"
out=${CAPTURE_DIR:-$here/captures}
[ $# -ge 1 ] || { echo "usage: $0 MOVE [MOVE...]   (one recording per move; repeat a name to take it again)"; exit 2; }

pid=$(for p in /proc/[0-9]*; do tr '\0' ' ' < "$p/cmdline" 2>/dev/null | grep -qi 'acbmp\.exe' && grep -qi 'acbmp\.exe' "$p/maps" 2>/dev/null && basename "$p"; done | head -1)
[ -n "$pid" ] || { echo "no ACBMP.exe process found: start the game under Wine first"; exit 1; }
echo "game pid $pid"

scope=$(cat /proc/sys/kernel/yama/ptrace_scope 2>/dev/null || echo 0)
if [ "$scope" != 0 ] && ! getcap "$bin" 2>/dev/null | grep -q cap_sys_ptrace; then
  echo "kernel.yama.ptrace_scope = $scope and acb-trace has no cap_sys_ptrace: reading the game will fail."
  echo "fix: sudo sysctl kernel.yama.ptrace_scope=0    (or: sudo setcap cap_sys_ptrace+ep $bin)"
  exit 3
fi

mkdir -p "$out"
for move in "$@"; do
  n=1; while [ -e "$out/$move-$n.csv" ]; do n=$((n+1)); done
  f="$out/$move-$n.csv"
  read -r -p "[$move] walk to the start spot (./acb-trace where --to X,Y,Z,YAW), then press Enter to START... " _
  "$bin" record --pid "$pid" --out "$f" &
  rec=$!
  read -r -p "[$move] recording -> $f   press Enter to STOP... " _
  kill -USR1 "$rec" 2>/dev/null; wait "$rec"
  echo "[$move] saved $f ($(($(wc -l < "$f") - 1)) rows)"
done
echo "done: send back the $out folder"
