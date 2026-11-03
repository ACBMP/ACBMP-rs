#!/usr/bin/env bash
# usage: tools/manhunt.sh [PLAY_SECONDS [NETSIM [extra serve args]]]
# Localhost 2v2 Manhunt: acbmp serve --mode manhunt + four headless viewers (a, c = team 0,
# b, d = team 1; team 0 hunts in round 1) over netsim loss/latency (default 5 % loss,
# 75 ms each way = 150 ms round trip). Every viewer runs the `vip` test bot: while its team
# hunts it runs at the nearest living player of the other team pressing kill; the prey stand
# at their team spawn. Rounds are shortened (intro and recap 2 s). Expect: hunter kills
# scored (100 points each) in both rounds, "round 2: team 1 attacks", 0 desync and 0 match
# desync on all four clients. Binds port 7953 (escort.sh 7951, assassinate.sh 7952).
# Run from the repo root after cargo build --release.
export ACBMP_SCHEMAS=~/Music/anvilforge-py/src/anvilforge/schemas ACBMP_MULTI=~/Music/ACB/multi ACBMP_EXE=~/Music/ACB/ACBMP.exe
M=~/Music/ACB/multi; O=${ESCORT_OUT:-$(mktemp -d)}; mkdir -p $O
play=${1:-40}; sim=${2:-loss=5%,delay=75ms,jitter=10ms}; shift $(( $# < 2 ? $# : 2 ))
./target/release/acbmp serve $M/DataPC_AC2MP_SanDonato.forge --port 7953 --players 4 --mode manhunt --time-limit $play --intro 2 --recap 2 "$@" > $O/serve.txt 2>&1 &
sleep 0.5
v() { WAYLAND_DISPLAY=wayland-1 XDG_RUNTIME_DIR=/run/user/1000 ACBMP_TRACE=1 timeout 400 ./target/release/acb-viewer $M/DataPC_AC2MP_SanDonato.forge --connect 127.0.0.1:7953 --name $1 --netsim $sim --character AC2MP_ID07_MainTemplate_Player --play --exe $ACBMP_EXE --input "0=vip" --fps $2 --capture $O/$1.png --capture-after 600 > $O/$1.txt 2>&1; }
v a 60 &
sleep 0.3; v b 144 &
sleep 0.3; v c 60 &
sleep 0.3; v d 60 &
wait
echo "logs in $O"
grep -E 'serve: (escort|result|sent|tick .* (VIP .* killed|round|team .* scores|escort over))' $O/serve.txt | head -40
echo "VIP kills (server): $(grep -c 'serve: tick .* VIP .* killed' $O/serve.txt), VIP kill scores: $(grep -cE 'serve: tick .* scores 100$' $O/serve.txt)"
for n in a b c d; do echo "== $n: $(grep -c '^net: event .* VIP .* killed' $O/$n.txt) VIP kill events"; grep -E '^net: tick|match over' $O/$n.txt | head -3; grep -E '^hud tick .*round' $O/$n.txt | awk 'NR%15==1' | head -3; grep '^hud: ' $O/$n.txt; done
