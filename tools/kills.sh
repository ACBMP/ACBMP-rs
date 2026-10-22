#!/usr/bin/env bash
# Scripted San Donato kill variants (free play, body 1 = the idle NPC at
# ~38,20) at 60 and 144 fps; prints the grant, the kill start and the final
# player line of each run. Expected (both rates):
#   air  AirDrop2Ground, start set 6 at 36.50 20.20 5.00, lead 1.00, player at 38.01 19.65 0.02
#   run  Run2Ground, start set 5 at 35.58 20.40 0.02, lead 0.30, player at 37.21 20.19 0.02
#   hay  runs into haystack 13 (24.62,7), body 1 placed at 24.62,9.4: Haystack2Ground Front,
#        start set 10 at 24.53 7.05 0.00, lead 0.40, player at 24.47 9.36 0.05
set -u
: "${ACBMP_SCHEMAS:=$HOME/Music/anvilforge-py/src/anvilforge/schemas}" "${ACBMP_EXE:=$HOME/Music/ACB/ACBMP.exe}"
export ACBMP_SCHEMAS ACBMP_EXE
M=${ACBMP_MULTI:-$HOME/Music/ACB/multi}
out=$(mktemp -d)
runs=(
  "air|36.5,20.2,5,-90|0=e 1="
  "run|31,20.4,0.02,-90|0=w,shift,e 2.5="
  "hay|21,7,0,-90,24.62,9.4,0,180|0=w,shift 0.8= 2.5=e 2.6="
)
for fps in 60 144; do
  for r in "${runs[@]}"; do
    IFS='|' read -r name at input <<<"$r"
    echo "== $name $fps fps"
    WAYLAND_DISPLAY=${WAYLAND_DISPLAY:-wayland-1} XDG_RUNTIME_DIR=${XDG_RUNTIME_DIR:-/run/user/1000} ACBMP_TRACE=1 \
      timeout 120 ./target/release/acb-viewer "$M/DataPC_AC2MP_SanDonato.forge" \
      --character AC2MP_ID07_MainTemplate_Player --count 2 --wander 0 --play --exe "$ACBMP_EXE" \
      --at="$at" --input "$input" --fps "$fps" --capture "$out/$name.png" --capture-after 5 2>&1 \
      | grep -E '^kill: (Kill body|start)|^player at' | tail -3
  done
done
rm -rf "$out"
