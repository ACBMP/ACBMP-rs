#!/usr/bin/env bash
# Scripted San Donato regression runs (climb, thin railing, parapet) at 60 and
# 144 fps; prints each run's final player line. Expected (both rates):
#   climb    -10.60 39.67 2.15
#   railing   38.60 -54.38 1.01
#   parapet  -10.29 -0.33 6.59
set -u
: "${ACBMP_SCHEMAS:=$HOME/Music/anvilforge-py/src/anvilforge/schemas}" "${ACBMP_EXE:=$HOME/Music/ACB/ACBMP.exe}"
export ACBMP_SCHEMAS ACBMP_EXE
M=${ACBMP_MULTI:-$HOME/Music/ACB/multi}
out=$(mktemp -d)
runs=(
  "climb|-18,44.6,2.22,-97|0=w,shift 1.5= 2.5=d 8= 8.5=s 8.8=|11"
  "railing|34.5,-54.38,0.1,-90|0=w,shift 1.1=|4"
  "parapet|-13,0,6,-97|0=w,shift 0.9=|4"
)
for fps in 60 144; do
  for r in "${runs[@]}"; do
    IFS='|' read -r name at input after <<<"$r"
    line=$(WAYLAND_DISPLAY=${WAYLAND_DISPLAY:-wayland-1} XDG_RUNTIME_DIR=${XDG_RUNTIME_DIR:-/run/user/1000} \
      timeout 120 ./target/release/acb-viewer "$M/DataPC_AC2MP_SanDonato.forge" \
      --character AC2MP_ID07_MainTemplate_Player --count 1 --play --exe "$ACBMP_EXE" \
      --at="$at" --input "$input" --fps "$fps" --capture "$out/$name.png" --capture-after "$after" 2>&1 \
      | grep '^player at' | tail -1)
    printf '%-8s %3s fps  %s\n' "$name" "$fps" "$line"
  done
done
rm -rf "$out"
