#!/usr/bin/env bash
# Scripted San Donato regression runs (climb, thin railing, parapet, wall
# jumps, ledge/beam-end stops, damage roll) at 60 and
# 144 fps; prints each run's final player line. Expected (both rates):
#   climb    -10.60 39.67 2.15
#   railing   38.60 -54.38 1.01
#   parapet  -10.29 -0.33 6.59
#   eject    -14.86 43.99 2.25   (hang, S+F: back eject, lands 1.28 m out)
#   sidejump -22.85 42.05 5.51   (hang at a dead end, Shift+A: 1.47 m gap)
#   leap     5.21 13.32 6.21     (climb, W+F: 2.4 m up to a hang)
#   pulldown -13.58 43.99 5.02   (walk to a roof edge: stops; W+F: hangs)
#   beamend  38.60 -55.12 1.01   (walk along a railing: stops at its end)
#   roll     34.46 20.40 0.02    (9.5 m fall, W held: damage roll, 30 %)
set -u
: "${ACBMP_SCHEMAS:=$HOME/Music/anvilforge-py/src/anvilforge/schemas}" "${ACBMP_EXE:=$HOME/Music/ACB/ACBMP.exe}"
export ACBMP_SCHEMAS ACBMP_EXE
M=${ACBMP_MULTI:-$HOME/Music/ACB/multi}
out=$(mktemp -d)
runs=(
  "climb|-18,44.6,2.22,-97|0=w,shift 1.5= 2.5=d 8= 8.5=s 8.8=|11"
  "railing|34.5,-54.38,0.1,-90|0=w,shift 1.1=|4"
  "parapet|-13,0,6,-97|0=w,shift 0.9=|4"
  "eject|-18,44.6,2.22,-97|0=w,shift 1.5= 3=s,f 3.3=|6"
  "sidejump|-24.0,43.3,3.8,180|0=w,shift 1.5= 2.5=a,shift 2.7=|5"
  "leap|5.21,14.8,3,180|0=w,shift 1.0= 3=w,f 3.2=|5"
  "pulldown|-12.0,43.99,6.5,90|0=w 3=w,f 3.2= 5=|6"
  "beamend|34.5,-54.38,0.1,-90|0=w,shift 1.1= 2=d 5=|6"
  "roll|31,20.4,9.5,-90|0=w 1.5=|3"
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
