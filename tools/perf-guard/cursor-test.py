#!/usr/bin/env python3
"""cursor-test.py <secs>: the pointer driven in a circle at about 120 Hz through Hyprland's socket; what the compositor's process and the GPU spend meanwhile."""
import socket,time,math,os,sys,subprocess,json
sig=os.environ['HYPRLAND_INSTANCE_SIGNATURE']
path=f'/run/user/1000/hypr/{sig}/.socket.sock'
secs=float(sys.argv[1]) if len(sys.argv)>1 else 15
def req(cmd):
    s=socket.socket(socket.AF_UNIX); s.connect(path); s.sendall(cmd.encode()); out=s.recv(4096); s.close(); return out
def hypr_pid():
    for p in os.listdir('/proc'):
        if p.isdigit():
            try:
                if open(f'/proc/{p}/comm').read().startswith('.Hyprland-wrapp') and open(f'/proc/{p}/cmdline').read().find('watchdog')>0: return p
            except OSError: pass
pid=hypr_pid(); hz=os.sysconf('SC_CLK_TCK')
def cpu():
    f=open(f'/proc/{pid}/stat').read().rsplit(')',1)[1].split(); return int(f[11])+int(f[12])
c0=cpu(); t0=time.time(); n=0
while time.time()-t0<secs:
    a=(time.time()-t0)*3
    x=int(1000+500*math.cos(a)); y=int(620+350*math.sin(a))
    req(f'/dispatch hl.dsp.cursor.move({{ x = {x}, y = {y} }})'); n+=1
    time.sleep(1/130)
dt=time.time()-t0
print(f'{n/dt:.0f} moves/s, Hyprland {(cpu()-c0)/hz/dt*100:.1f}% of a CPU')
