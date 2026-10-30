#!/usr/bin/env python3
"""vblank.py <secs> [crtc]: how often the panel really refreshes: DRM vblank timestamps on /dev/dri/card1."""
import os,sys,fcntl,struct,time,collections
secs=float(sys.argv[1]); pipe=int(sys.argv[2]) if len(sys.argv)>2 else 0
fd=os.open('/dev/dri/card1',os.O_RDWR)
IOC=0xC018643A; REL=0x1; SECOND=0x20000000
t=REL|(SECOND if pipe==1 else 0)|((pipe<<1)&0x3e if pipe>=2 else 0)
stamps=[]; end=time.time()+secs
while time.time()<end:
    buf=bytearray(struct.pack('IIQQ',t,1,0,0))
    fcntl.ioctl(fd,IOC,buf,True)
    _,seq,sec,usec=struct.unpack('IIqq',bytes(buf))
    stamps.append(sec+usec*1e-6)
g=sorted((b-a)*1e3 for a,b in zip(stamps,stamps[1:]) if b>a)
if g:
    print(f'{len(g)/secs:6.1f} refreshes/s, gap median {g[len(g)//2]:.2f} ms  p95 {g[len(g)*95//100]:.2f}  max {g[-1]:.2f}')
