#!/usr/bin/env bash
# Replay the capture-protocol moves (docs/capture-protocol.md) and write our trace CSVs:
#   tools/trace-ours.sh OUTDIR [move...] [--fps N]
# Then: tools/tracecmp.py retail/<move>-1.csv OUTDIR/<move>.csv
set -u
: "${ACBMP_SCHEMAS:=$HOME/Music/anvilforge-py/src/anvilforge/schemas}" "${ACBMP_EXE:=$HOME/Music/ACB/ACBMP.exe}"
export ACBMP_SCHEMAS ACBMP_EXE
M=${ACBMP_MULTI:-$HOME/Music/ACB/multi}
out=${1:?usage: trace-ours.sh OUTDIR [move...] [--fps N]}; shift
fps=60; want=()
while (($#)); do
  if [[ $1 == --fps ]]; then fps=$2; shift 2; else want+=("$1"); shift; fi
done
# name|x,y,z,yaw|input|seconds   (same spots as tools/gates.sh where the move exists there)
moves=(
  "walk|-18,44.6,2.22,-97|0=w 2=|4"
  "run|-18,44.6,2.22,-97|0=w,shift 2=|4"
  "sprint|-18,44.6,2.22,-97|0=w,space 2=|4"
  "turn180|-18,44.6,2.22,-97|0=w,shift 1=s,shift 2=|4"
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
mkdir -p "$out"
for r in "${moves[@]}"; do
  IFS='|' read -r name at input after <<<"$r"
  if ((${#want[@]})) && [[ ! " ${want[*]} " == *" $name "* ]]; then continue; fi
  ACBMP_TRACE="csv:$out/$name.csv" WAYLAND_DISPLAY=${WAYLAND_DISPLAY:-wayland-1} \
    XDG_RUNTIME_DIR=${XDG_RUNTIME_DIR:-/run/user/1000} timeout 120 ./target/release/acb-viewer \
    "$M/DataPC_AC2MP_SanDonato.forge" --character AC2MP_ID07_MainTemplate_Player --count 1 --play \
    --exe "$ACBMP_EXE" --at="$at" --input "$input" --fps "$fps" \
    --capture "$out/.$name.png" --capture-after "$after" >/dev/null 2>&1
  rm -f "$out/.$name.png"
  printf '%-9s %s rows\n' "$name" "$(($(wc -l <"$out/$name.csv") - 1))"
done
