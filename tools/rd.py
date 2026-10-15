#!/usr/bin/env python3
"""rd.py <binary> <fmt> <vmaddr>...: read struct values at VM addresses of a Mach-O."""
import struct, subprocess, sys, re
binp, fmt = sys.argv[1], sys.argv[2]
hdr = subprocess.run(['llvm-objdump','--macho','--private-headers',binp],capture_output=True,text=True).stdout
segs=[]
cur={}
for l in hdr.splitlines():
    l=l.strip()
    m=re.match(r'(vmaddr|vmsize|fileoff|filesize)\s+(0x[0-9a-f]+|\d+)',l)
    if l.startswith('cmd LC_SEGMENT'): cur={}
    if m:
        cur[m.group(1)]=int(m.group(2),0)
        if m.group(1)=='filesize': segs.append(dict(cur))
data=open(binp,'rb').read()
for a in sys.argv[3:]:
    va=int(a,16)
    for s in segs:
        if s['vmaddr']<=va<s['vmaddr']+s['filesize']:
            off=s['fileoff']+va-s['vmaddr']
            print(a, struct.unpack_from('<'+fmt, data, off))
            break
    else: print(a,'unmapped')
