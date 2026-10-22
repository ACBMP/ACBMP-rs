#!/usr/bin/env bash
# usage: tools/duel.sh [FPS_A FPS_B INPUT_A INPUT_B [extra serve args]]
# Localhost duel: acbmp serve + two headless viewers (A at 60 fps, B at 144) over netsim
# loss/latency. Expect: kill rejected at tick 30, A kills B around tick 400-430
# (scripted keys are fed in real time, so the tick varies a little), server prints
# "result after 20.0 s: a 100, b 0 (a wins)". Run from the repo root after cargo build --release.
export ACBMP_SCHEMAS=~/Music/anvilforge-py/src/anvilforge/schemas ACBMP_MULTI=~/Music/ACB/multi ACBMP_EXE=~/Music/ACB/ACBMP.exe
M=~/Music/ACB/multi; O=$(mktemp -d)
if [ $# -ge 4 ]; then fa=$1 fb=$2 ia=$3 ib=$4; shift 4
else fa=60 fb=144 ia="0.5=e 0.8= 4=w,shift 6.2=w 6.8=e 7.2=" ib=""; fi
./target/release/acbmp serve $M/DataPC_AC2MP_SanDonato.forge --port 7950 --players 2 --at=31,20.4,0.02,-90 --at=41,20.4,0.02,90 --time-limit 20 "$@" > $O/serve.txt 2>&1 &
sleep 0.5
v() { WAYLAND_DISPLAY=wayland-1 XDG_RUNTIME_DIR=/run/user/1000 ACBMP_TRACE=1 timeout 200 ./target/release/acb-viewer $M/DataPC_AC2MP_SanDonato.forge --connect 127.0.0.1:7950 --name $1 --netsim loss=2%,delay=40ms,jitter=5ms --character AC2MP_ID07_MainTemplate_Player --play --exe $ACBMP_EXE --input "$2" --fps $3 --capture $O/$1.png --capture-after 60 > $O/$1.txt 2>&1; }
v a "$ia" $fa & 
sleep 0.3
v b "$ib" $fb &
wait
grep -E 'serve: (result|sent)' $O/serve.txt
for n in a b; do echo "== $n"; grep -E '^hud|^net: event|match over|^player at|^net: tick' $O/$n.txt | grep -v 'hud tick' | head -20; done
