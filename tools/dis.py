#!/usr/bin/env python3
"""dis.py <binary> <symbol substring> : disassemble a function with stub calls named."""
import re, subprocess, sys, functools
binp, needle = sys.argv[1], sys.argv[2]
@functools.lru_cache(None)
def stubs():
    out = subprocess.run(['llvm-objdump','--macho','--indirect-symbols',binp],capture_output=True,text=True).stdout
    m = {}
    for l in out.splitlines():
        p = l.split()
        if len(p)==3 and p[0].startswith('0x'):
            m[int(p[0],16)] = p[2]
    return m
syms = subprocess.run(f"llvm-nm -n {binp} | c++filt -_", shell=True, capture_output=True, text=True).stdout.splitlines()
addrs=[]
for i,l in enumerate(syms):
    p=l.split(' ',2)
    if len(p)==3 and p[1] in ("T","t") and p[0]:
        addrs.append((int(p[0],16),p[2]))
for i,(a,n) in enumerate(addrs):
    if needle in n:
        end = addrs[i+1][0] if i+1<len(addrs) else a+0x400
        print(f'== {a:#x} {n}')
        d = subprocess.run(['llvm-objdump','-d','--no-show-raw-insn',f'--start-address={a:#x}',f'--stop-address={end:#x}',binp],capture_output=True,text=True).stdout
        st = stubs()
        names = subprocess.run(['c++filt','-_'], input='\n'.join(st.values()), capture_output=True, text=True).stdout.splitlines()
        dm = dict(zip(st.keys(), names))
        for l in d.splitlines()[6:]:
            m = re.search(r'call[l]?\s+(0x[0-9a-f]+)', l)
            if m and int(m.group(1),16) in dm:
                l = l.split('<')[0] + ' <' + dm[int(m.group(1),16)] + '>'
            print(l)
        if len(sys.argv) < 4: break
