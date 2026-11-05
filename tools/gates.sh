#!/usr/bin/env bash
# Scripted San Donato regression runs (climb, thin railing, parapet, wall
# jumps, ledge/beam-end stops, damage roll) at 60 and
# 144 fps; prints each run's final player line. Expected (both rates):
#   climb    -10.60 39.67 2.15   (S drops off the hang: 0.47 s without grabs, so
#                                 not the hold it let go of, and down to the ground)
#   railing   38.60 -54.38 1.01
#   parapet  -10.29 -0.08 6.59   (a jump up, the stick let go at 0.4 s on the top;
#                                 gaits are instant, so all these inputs come
#                                 sooner than they did with the speed ramp)
#   eject    -14.03 44.16 2.21   (hang, S+F: retail rebound, a targeted jump to
#                                 the floor at the wall's foot, 0.45 m out)
#   rebound  -46.79 13.18 -0.01  (10 m hang, S+F: no target in reach, retail
#                                 farthest foothold 8 m out 3 m down, then drops)
#   sidejump -22.85 42.05 5.51   (hang at a dead end, Shift+A: 1.47 m gap)
#   leap     5.21 13.58 7.29     (hang, W+F: 1.08 m up to the next hang)
#   pulldown -13.58 43.99 5.02   (walk to a roof edge: stops; W+F: hangs)
#   beamend  38.60 -55.11 1.01   (walk along a railing: stops at its end)
#   roll     33.74 20.40 0.02    (9.5 m fall, W held: damage roll, 30 %)
#   pilotis  -41.71 10.61 6.90   (0.6 m drop 0.3 m beside a post: retail
#                                 CheckForPilotis, onto its centre)
set -u
: "${ACBMP_SCHEMAS:=$HOME/Music/anvilforge-py/src/anvilforge/schemas}" "${ACBMP_EXE:=$HOME/Music/ACB/ACBMP.exe}"
export ACBMP_SCHEMAS ACBMP_EXE
M=${ACBMP_MULTI:-$HOME/Music/ACB/multi}
out=$(mktemp -d)
runs=(
  "climb|-18,44.6,2.22,-97|0=w,shift 1.5= 2.5=d 8= 8.5=s 8.8=|11"
  "railing|34.5,-54.38,0.1,-90|0=w,shift 0.3=w,shift,space 0.32=w,shift 0.8=|4"
  "parapet|-13,0,6,-97|0=w,shift 0.1=w,shift,space 0.12=w,shift 0.4=|4"
  "eject|-18,44.6,2.22,-97|0=w,shift 1.5= 3=s,f 3.3=|6"
  "rebound|-46.81,21.36,10.6,0|0= 1.5=s,f 1.8=|6"
  "sidejump|-24.0,43.3,3.8,180|0=w,shift 0.8= 2.5=a,shift 2.7=|5"
  "leap|5.21,14.8,3,180|0=w,shift 0.5= 3=w,f 3.2=|5"
  "pulldown|-12.0,43.99,6.5,90|0=w 3=w,f 3.2= 5=|6"
  "beamend|34.5,-54.38,0.1,-90|0=w,shift 0.3=w,shift,space 0.32=w,shift 0.8= 2=d 5=|6"
  "roll|31,20.4,9.5,-90|0=w 1.5=|3"
  "pilotis|-41.81,10.30,7.5,-90|0=|2"
)
for fps in 60 144; do
  for r in "${runs[@]}"; do
    IFS='|' read -r name at input after <<<"$r"
    line=$(WAYLAND_DISPLAY=${WAYLAND_DISPLAY:-wayland-1} XDG_RUNTIME_DIR=${XDG_RUNTIME_DIR:-/run/user/1000} \
      timeout 120 ./target/release/acb-viewer "$M/DataPC_AC2MP_SanDonato.forge" \
      --character AC2MP_ID07_MainTemplate_Player --count 1 --crowd 0 --play --exe "$ACBMP_EXE" \
      --at="$at" --input "$input" --fps "$fps" --capture "$out/$name.png" --capture-after "$after" 2>&1 \
      | grep '^player at' | tail -1)
    printf '%-8s %3s fps  %s\n' "$name" "$fps" "$line"
  done
done
rm -rf "$out"
